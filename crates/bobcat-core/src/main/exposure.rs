//! Lynx exposure (曝光): which elements a page registered for exposure are on
//! screen, told to script as an element's `uiappear`/`uidisappear` and as the
//! global `exposure`/`disexposure` record lists.
//!
//! Exposure is a Lynx-only extension (standards bucket 2): no W3C feature has
//! its attributes, its events or its records. It lives here rather than in
//! `dom`, and `dom` is not changed for it: each registered element is one
//! W3C intersection observer of `dom`'s (`dom::IntersectionEventHandler`),
//! and this module turns what those observers deliver into exposure
//! transitions.
//!
//! # The rulings it is built on (user, 2026-10-11)
//!
//! **Geometry is the W3C primitive's, unchanged.** The root is the viewport,
//! a target is clipped through every clipping ancestor on its chain, and an
//! element is *visible* when it is intersecting and its intersection ratio —
//! the share of it that is finally visible — is at least its
//! `exposure-area`. That is neither web-core's root, the nearest scroll
//! container (`ExposureServices.ts:152-164`), nor native's per-ancestor rect
//! tests (`LynxUIExposure.m:367-432`, `UIExposure.java:339-387`). Because the
//! primitive already clips through every ancestor, `enable-exposure-ui-clip`
//! has nothing left to choose and is ignored.
//!
//! **Margins have their documented, native meaning.**
//! `exposure-screen-margin-*` always applies: a positive margin grows the
//! screen rect on that side and a negative one shrinks it.
//! `exposure-ui-margin-*` grows the element's own rect, and applies only when
//! the element's `enable-exposure-ui-margin` is true or, where the element
//! names none, when the page's `enableExposureUIMargin` is
//! (`LynxBaseUI.java:1583-1591`; `LynxUIExposure.m:266-334`,
//! `UIExposure.java:262-296`). Both are folded into the observer's one
//! `rootMargin`: growing the element's bottom edge by `b` is growing the
//! root's top edge by `b`, so the ui margin of each side lands on the root
//! margin of the opposite side. web-core folds the same sides, with signs
//! that cancel a ui margin rather than add it, and has no page switch
//! (`ExposureServices.ts:141-151`).
//!
//! **`stopExposure`/`resumeExposure` are native's** (`LynxUIExposure.m:538-569`,
//! `UIExposure.java:478-505`). Stopping drops every observer, so nothing is
//! detected and no `uiappear` fires; with `sendEvent` (its default) every
//! element recorded as exposed gets one `disexposure` record and the record
//! is cleared, without it nothing is sent and the record is kept. Resuming
//! observes every registered element again, registrations made while stopped
//! included, and whatever is visible then and not recorded as exposed is
//! exposed again, `uiappear` included. web-core's switch only gates its
//! global records and keeps observing (`ExposureServices.ts:278-302`).
//!
//! **There is no timer.** Native polls on a display link and web-core batches
//! its records behind a 50 ms timeout (`ExposureServices.ts:246-275`). Here
//! the transitions are delivered by the page's intersection delivery entry,
//! the one the epilogue posts after an update that queued entries
//! (`docs/runtime-architecture.md`): first each `uiappear`/`uidisappear`, in
//! the order the transitions formed, then one `exposure` list, then one
//! `disexposure` list, web-core's order. `lynx.setObserverFrameRate` has no
//! rate to set.
//!
//! # Where the references differ, and what is chosen
//!
//! web-core is the default; each choice below is recorded as a deviation.
//!
//! - **Records exist only for an element whose `exposure-id` is present and non-empty**, as
//!   native's are; web-core also sends one with a `null` id for an element that only has a
//!   listener.
//! - **An element is one record, whoever else has its id**: two elements with the same id and scene
//!   are two records (web-core keys its sets by element).
//! - **A freed element that was exposed sends one `disexposure`**, as native's next pass does
//!   (web-core sends nothing). An element that is unlinked but alive leaves through the primitive:
//!   it is no longer rendered, so its update reports it not intersecting.
//! - **Teardown sends a record and no element event.** Removing an exposed element's `exposure-id`,
//!   its last listener, or changing its id sends a `disexposure` record carrying the id it was
//!   exposed under and fires no `uidisappear`, as web-core's `sendAppearEvent = false` does
//!   (`ExposureServices.ts:86-104`); so does `stopExposure` with `sendEvent`. A resume that finds
//!   an element visible fires `uiappear` as well as the record, as native does, because resuming is
//!   an ordinary update here.
//! - **The primitive's edge cases stand**: a zero-area element that is intersecting has ratio 1, an
//!   element that only touches an edge is intersecting, and `visibility: hidden` or `opacity: 0`
//!   hides nothing from it — where native reports a zero-sized element hidden
//!   (`LynxUIExposure.m:373-375`), needs strict overlap and walks `isVisible`.
//! - **Lengths**: `<n>px`, `<n>rpx` (1rpx is the viewport width over 750, stylo's rule), and a bare
//!   number as px (`UnitUtils.java:59-86`). `<n>%` on a screen margin is the intersection
//!   observer's root percentage: top and bottom of the viewport's height, left and right of its
//!   width. `%` on a ui margin is of the element's own size natively, which a root margin cannot
//!   express, so it is 0. Anything else is 0, as web-core's `convertLengthToPx` answers for what it
//!   cannot read. A negative length is kept.
//! - **`exposure-area`** is `parseFloat(value) / 100`, web-core's (`ExposureServices.ts:107-108`):
//!   `"50%"` and `"50"` are both one half. It is clamped to `[0, 1]`, and what does not parse is 0.
//! - **`exposure-scene`** is `""` when absent.
//!
//! # How a registration is held
//!
//! An element is registered while it has a non-empty `exposure-id` or the
//! realm has filed a `uiappear`/`uidisappear` handler on it, which the
//! realm reports through the `exposureEvents` host member. Each
//! registration owns one observer — root the viewport, one threshold, the
//! element's `exposure-area`, and its folded margins — whose handler,
//! [`ExposureHandler`], compares what the primitive reports with what was
//! last recorded and queues a [`Transition`] where they differ. One
//! threshold at the area is enough: the primitive queues an entry exactly
//! when the pair (threshold index, intersecting) changes, which is exactly
//! when "intersecting with a ratio of at least the area" can change.
//!
//! Every attribute change re-arms: the observer is dropped and created again
//! from the attributes as they are now. The first update after an observe
//! always reports the element, so an element whose visibility did not move
//! reports what was already recorded and queues nothing.
//!
//! The handler is bound to its element, so the primitive drops it when the
//! element is freed; that drop is how a freed element is noticed. Every drop
//! this module makes itself clears the registration's observer first, so the
//! handler's `Drop` tells the two apart. That `Drop` borrows the shared state
//! and runs inside `Document::drop_intersection_observer` and inside an
//! element's free (`Document::drop_element`), so no borrow of the shared
//! state is ever held across a `Document` call.

use std::cell::RefCell;
use std::rc::Rc;

use dom::{
    IntersectionEventHandler, IntersectionObserverEntry, IntersectionObserverId, MarginLength,
    NodeId, RootMargin,
};
use rustc_hash::FxHashMap;

use super::tree::LynxDocument;

/// The attribute whose presence, non-empty, asks for global records.
const EXPOSURE_ID: &str = "exposure-id";
/// What a record names besides the id; `""` when absent.
const EXPOSURE_SCENE: &str = "exposure-scene";
/// The visible share an element needs, as `parseFloat(value) / 100`.
const EXPOSURE_AREA: &str = "exposure-area";
/// The per-element switch that lets the `exposure-ui-margin-*` attributes
/// apply, over the page's `enableExposureUIMargin`.
const ENABLE_UI_MARGIN: &str = "enable-exposure-ui-margin";
/// The screen margins, top, right, bottom, left.
const SCREEN_MARGINS: [&str; 4] = [
    "exposure-screen-margin-top",
    "exposure-screen-margin-right",
    "exposure-screen-margin-bottom",
    "exposure-screen-margin-left",
];
/// The element's own margins, top, right, bottom, left.
const UI_MARGINS: [&str; 4] = [
    "exposure-ui-margin-top",
    "exposure-ui-margin-right",
    "exposure-ui-margin-bottom",
    "exposure-ui-margin-left",
];

/// Whether writing or removing `name` can change an element's exposure
/// registration: every `exposure-` attribute, and the ui-margin switch.
///
/// A prefix test rather than the list, because it is asked on every
/// attribute write and a name that matches without meaning anything only
/// costs a re-arm of an element that is registered, and nothing for one that
/// is not.
pub(crate) fn is_exposure_attribute(name: &str) -> bool {
    name.starts_with("exposure-") || name == ENABLE_UI_MARGIN
}

/// One element's exposure moving: it became visible (`appeared`) or stopped
/// being so, or a teardown took it out of the exposed record.
///
/// The id and the scene are copied when the transition forms, because the
/// element may be freed, or its attributes rewritten, before the delivery
/// entry runs: a `disexposure` names what the element was exposed under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Transition {
    pub(crate) node: NodeId,
    pub(crate) appeared: bool,
    /// The `exposure-id` this transition is recorded under: `None` for an
    /// element registered only by its listener, which gets no record.
    pub(crate) exposure_id: Option<String>,
    pub(crate) scene: String,
    /// Whether the element hears `uiappear`/`uidisappear` for it: false for
    /// a teardown and for `stopExposure`, which send records only.
    pub(crate) element_event: bool,
}

/// One registered element.
#[derive(Debug)]
struct Entry {
    /// The observer this registration owns. `None` while exposure is stopped
    /// and before the first arm, and cleared before every drop this module
    /// makes, which is what [`ExposureHandler`]'s `Drop` reads.
    observer: Option<IntersectionObserverId>,
    /// The realm has filed a `uiappear`/`uidisappear` handler on it.
    listens: bool,
    /// `exposure-id` when present and non-empty, copied so that a freed
    /// element still names it.
    exposure_id: Option<String>,
    scene: String,
    /// `exposure-area` as a ratio in `[0, 1]`.
    area: f64,
    /// Recorded as exposed: the last transition this registration queued
    /// was an appearance.
    exposed: bool,
}

/// What [`Exposure`] and every [`ExposureHandler`] share.
#[derive(Debug, Default)]
struct Shared {
    entries: FxHashMap<NodeId, Entry>,
    /// Pending, in the order they formed.
    transitions: Vec<Transition>,
}

/// Queues `transition` unless it can say nothing: one with no id and no
/// element event would deliver neither a record nor an event, and queued it
/// would still cost a delivery entry.
fn queue(transitions: &mut Vec<Transition>, transition: Transition) {
    if transition.exposure_id.is_some() || transition.element_event {
        transitions.push(transition);
    }
}

/// Records `entry` as visible or not, queueing the transition when that is a
/// change. `element_event` is whether the element hears it.
fn record(
    transitions: &mut Vec<Transition>,
    node: NodeId,
    entry: &mut Entry,
    visible: bool,
    element_event: bool,
) {
    if visible == entry.exposed {
        return;
    }
    entry.exposed = visible;
    queue(
        transitions,
        Transition {
            node,
            appeared: visible,
            exposure_id: entry.exposure_id.clone(),
            scene: entry.scene.clone(),
            element_event,
        },
    );
}

/// The handler of one registration's observer.
///
/// Bound to its element, so the primitive drops it when the element is
/// freed; see its `Drop`.
#[derive(Debug)]
struct ExposureHandler {
    node: NodeId,
    shared: Rc<RefCell<Shared>>,
}

impl IntersectionEventHandler<()> for ExposureHandler {
    /// Compares each entry with the recorded state and queues a transition
    /// where they differ. Touches no `Document` method, which is what lets it
    /// hold the shared borrow for the whole call.
    fn notify(
        &mut self,
        _document: &mut LynxDocument,
        observer: IntersectionObserverId,
        entries: Vec<IntersectionObserverEntry>,
    ) {
        let mut shared = self.shared.borrow_mut();
        let Shared {
            entries: registered,
            transitions,
        } = &mut *shared;
        let Some(entry) = registered.get_mut(&self.node) else {
            return;
        };
        // Only the registration's current observer speaks for it; a dropped
        // one cannot deliver, so this is a statement of the invariant more
        // than a filter.
        if entry.observer != Some(observer) {
            return;
        }
        for observed in entries {
            // A ratio is never negative, so an area of 0 asks for nothing
            // but intersection.
            let visible = observed.is_intersecting && observed.intersection_ratio >= entry.area;
            let element_event = entry.listens;
            record(transitions, self.node, entry, visible, element_event);
        }
    }

    fn bound_to(&self) -> Option<NodeId> {
        Some(self.node)
    }
}

impl Drop for ExposureHandler {
    /// The element was freed, when its registration still names an
    /// observer: every drop this module makes clears that first. A freed
    /// element that was exposed owes one `disexposure`, with no element
    /// event — there is no element left to hear one — and the registration
    /// goes.
    fn drop(&mut self) {
        let mut shared = self.shared.borrow_mut();
        let Shared {
            entries,
            transitions,
        } = &mut *shared;
        let freed = entries
            .get(&self.node)
            .is_some_and(|entry| entry.observer.is_some());
        if !freed {
            return;
        }
        if let Some(mut entry) = entries.remove(&self.node) {
            record(transitions, self.node, &mut entry, false, false);
        }
    }
}

/// A length one margin attribute names, before it is folded.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Length {
    Px(f32),
    /// As written: `20.0` is `20%`.
    Percent(f32),
}

/// One margin attribute's value: `<n>px`, `<n>rpx`, `<n>%`, or a bare `<n>`
/// as px, with surrounding white space ignored. What does not parse, and an
/// absent attribute, is `0px`.
///
/// `rpx` is resolved here, against `viewport_width`: stylo's rule, one rpx is
/// the viewport width over 750 (`vendor/stylo/style/values/specified/length.rs`,
/// `rpx_to_computed_value`), without its truncation to the app-unit grid.
fn parse_length(value: Option<&str>, viewport_width: f32) -> Length {
    let Some(value) = value.map(str::trim) else {
        return Length::Px(0.0);
    };
    let number = |text: &str| {
        text.trim()
            .parse::<f32>()
            .ok()
            .filter(|number| number.is_finite())
    };
    let length = if let Some(rpx) = value.strip_suffix("rpx") {
        number(rpx).map(|rpx| Length::Px(rpx * viewport_width / 750.0))
    } else if let Some(px) = value.strip_suffix("px") {
        number(px).map(Length::Px)
    } else if let Some(percent) = value.strip_suffix('%') {
        number(percent).map(Length::Percent)
    } else {
        number(value).map(Length::Px)
    };
    length.unwrap_or(Length::Px(0.0))
}

/// A ui margin in px. A percentage is of the element's own size natively,
/// which a root margin has no way to name, so it is 0.
const fn ui_px(length: Length) -> f32 {
    match length {
        Length::Px(px) => px,
        Length::Percent(_) => 0.0,
    }
}

/// One side of the root margin: the screen margin of that side plus the ui
/// margin of the opposite side.
///
/// A screen percentage stays a percentage of the viewport, resolved by the
/// primitive at every update, as long as no ui margin is added to it; a sum
/// of a percentage and a length is not something a root margin can carry, so
/// then the percentage counts as 0 and the ui margin is kept.
fn fold_side(screen: Length, ui: f32) -> MarginLength {
    match screen {
        Length::Px(px) => MarginLength::Px(px + ui),
        Length::Percent(percent) if ui == 0.0 => MarginLength::Percent(percent),
        Length::Percent(_) => MarginLength::Px(ui),
    }
}

/// The root margin an element's margins make: each side's screen margin,
/// plus — when ui margins apply — the opposite side's ui margin.
fn fold_margins(screen: [Length; 4], ui: [f32; 4]) -> RootMargin {
    let [top, right, bottom, left] = screen;
    let [ui_top, ui_right, ui_bottom, ui_left] = ui;
    RootMargin {
        top: fold_side(top, ui_bottom),
        right: fold_side(right, ui_left),
        bottom: fold_side(bottom, ui_top),
        left: fold_side(left, ui_right),
    }
}

/// `exposure-area` as a ratio: `parseFloat(value) / 100` (absent is `"0"`),
/// clamped to `[0, 1]`, and 0 for what does not parse.
fn parse_area(value: Option<&str>) -> f64 {
    let ratio = parse_float(value.unwrap_or("0")) / 100.0;
    if ratio.is_nan() {
        0.0
    } else {
        ratio.clamp(0.0, 1.0)
    }
}

/// ECMAScript's `parseFloat`: the longest prefix of `text`, past leading
/// white space, that is a decimal literal or `Infinity`, and `NaN` when there
/// is none — so `"50%"` reads as 50.
fn parse_float(text: &str) -> f64 {
    let text = text.trim_start();
    let bytes = text.as_bytes();
    let signed = matches!(bytes.first(), Some(b'+' | b'-'));
    let unsigned = &text[usize::from(signed)..];
    if unsigned.starts_with("Infinity") {
        return if bytes.first() == Some(&b'-') {
            f64::NEG_INFINITY
        } else {
            f64::INFINITY
        };
    }
    let digits = |from: usize| {
        bytes[from..]
            .iter()
            .take_while(|byte| byte.is_ascii_digit())
            .count()
    };
    let mut end = usize::from(signed);
    let integer = digits(end);
    end += integer;
    let mut fraction = 0;
    if bytes.get(end) == Some(&b'.') {
        fraction = digits(end + 1);
        // A lone `.` with no digit on either side is not part of a number.
        if integer + fraction > 0 {
            end += 1 + fraction;
        }
    }
    if integer + fraction == 0 {
        return f64::NAN;
    }
    if matches!(bytes.get(end), Some(b'e' | b'E')) {
        let mut exponent = end + 1;
        if matches!(bytes.get(exponent), Some(b'+' | b'-')) {
            exponent += 1;
        }
        let exponent_digits = digits(exponent);
        if exponent_digits > 0 {
            end = exponent + exponent_digits;
        }
    }
    text[..end].parse().unwrap_or(f64::NAN)
}

/// What one element's attributes ask for, read at an arm.
struct Attributes {
    exposure_id: Option<String>,
    scene: String,
    area: f64,
    margin: RootMargin,
}

impl Attributes {
    /// Reads `node`'s exposure attributes. `ui_margin_default` is the page's
    /// `enableExposureUIMargin`, which an element without its own
    /// `enable-exposure-ui-margin` follows; the element's own switch is on
    /// when present and not `"false"`, the reading every boolean attribute
    /// gets here (see `tree::dialog`'s `open`).
    fn read(document: &LynxDocument, node: NodeId, ui_margin_default: bool) -> Self {
        let element = document
            .get(node)
            .expect("an element is registered for exposure only while it is live");
        let width = document.viewport_size().width;
        let length = |name: &str| parse_length(element.attribute(name), width);
        let ui_margins = element
            .attribute(ENABLE_UI_MARGIN)
            .map_or(ui_margin_default, |value| value != "false");
        let ui = if ui_margins {
            UI_MARGINS.map(|name| ui_px(length(name)))
        } else {
            [0.0; 4]
        };
        Self {
            exposure_id: element
                .attribute(EXPOSURE_ID)
                .filter(|id| !id.is_empty())
                .map(str::to_owned),
            scene: element
                .attribute(EXPOSURE_SCENE)
                .unwrap_or_default()
                .to_owned(),
            area: parse_area(element.attribute(EXPOSURE_AREA)),
            margin: fold_margins(SCREEN_MARGINS.map(length), ui),
        }
    }
}

/// A document's exposure registrations, the switch `stopExposure` and
/// `resumeExposure` turn, and the transitions waiting for a delivery entry.
///
/// Owned by the realm's document slot beside the document, and created with
/// it, from the page's `enableExposureUIMargin`.
#[derive(Debug)]
pub(crate) struct Exposure {
    /// Shared with every registration's [`ExposureHandler`].
    shared: Rc<RefCell<Shared>>,
    /// False between `stopExposure` and `resumeExposure`.
    running: bool,
    /// The page's `enableExposureUIMargin`.
    ui_margin_default: bool,
}

impl Exposure {
    /// No registrations, detection running, and `ui_margin_default` as the
    /// page's `enableExposureUIMargin`.
    pub(crate) fn new(ui_margin_default: bool) -> Self {
        Self {
            shared: Rc::default(),
            running: true,
            ui_margin_default,
        }
    }

    /// An `exposure-*` attribute or `enable-exposure-ui-margin` of the live
    /// element `node` was written or removed: the registration follows the
    /// attributes as they are now.
    pub(crate) fn attribute_changed(&mut self, document: &mut LynxDocument, node: NodeId) {
        let listens = self
            .shared
            .borrow()
            .entries
            .get(&node)
            .is_some_and(|entry| entry.listens);
        self.update(document, node, listens);
    }

    /// The realm's `exposureEvents`: whether the live element `node` has a
    /// `uiappear`/`uidisappear` handler, reported on each change.
    pub(crate) fn set_listens(&mut self, document: &mut LynxDocument, node: NodeId, wants: bool) {
        self.update(document, node, wants);
    }

    /// Registers, re-arms or tears down `node`: registered while it has an
    /// `exposure-id` or a listener.
    fn update(&mut self, document: &mut LynxDocument, node: NodeId, listens: bool) {
        let attributes = Attributes::read(document, node, self.ui_margin_default);
        if attributes.exposure_id.is_some() || listens {
            self.arm(document, node, attributes, listens);
        } else {
            self.teardown(document, node);
        }
    }

    /// Registers `node`, or re-arms it, with `attributes`.
    ///
    /// An existing observer is dropped. A registration recorded as exposed
    /// under another id than the one it now has sends a `disexposure` under
    /// the old id and is recorded as not exposed, so the new id gets an
    /// exposure of its own. While detection runs, a fresh observer is
    /// created and observes the element.
    fn arm(
        &mut self,
        document: &mut LynxDocument,
        node: NodeId,
        attributes: Attributes,
        listens: bool,
    ) {
        let Attributes {
            exposure_id,
            scene,
            area,
            margin,
        } = attributes;
        let previous = {
            let mut shared = self.shared.borrow_mut();
            let Shared {
                entries,
                transitions,
            } = &mut *shared;
            let entry = entries.entry(node).or_insert(Entry {
                observer: None,
                listens,
                exposure_id: None,
                scene: String::new(),
                area: 0.0,
                exposed: false,
            });
            let previous = entry.observer.take();
            if entry.exposure_id != exposure_id {
                record(transitions, node, entry, false, false);
            }
            entry.listens = listens;
            entry.exposure_id = exposure_id;
            entry.scene = scene;
            entry.area = area;
            previous
        };
        if let Some(observer) = previous {
            document.drop_intersection_observer(observer);
        }
        if !self.running {
            return;
        }
        let observer = document.create_intersection_observer(
            Box::new(ExposureHandler {
                node,
                shared: Rc::clone(&self.shared),
            }),
            None,
            margin,
            vec![area],
        );
        document.observe_intersection(observer, node);
        self.shared
            .borrow_mut()
            .entries
            .get_mut(&node)
            .expect("the registration was just written")
            .observer = Some(observer);
    }

    /// Unregisters `node`: its observer is dropped, and if it was recorded
    /// as exposed it sends a `disexposure` under the id it was exposed under,
    /// with no element event.
    fn teardown(&mut self, document: &mut LynxDocument, node: NodeId) {
        let observer = {
            let mut shared = self.shared.borrow_mut();
            let Shared {
                entries,
                transitions,
            } = &mut *shared;
            let Some(mut entry) = entries.remove(&node) else {
                return;
            };
            record(transitions, node, &mut entry, false, false);
            entry.observer
        };
        if let Some(observer) = observer {
            document.drop_intersection_observer(observer);
        }
    }

    /// `lynx.stopExposure({sendEvent})` (`on` false) and
    /// `lynx.resumeExposure()` (`on` true), as native runs them. Stopping a
    /// stopped document and resuming a running one do nothing.
    ///
    /// Stopping drops every observer; with `send_event` every registration
    /// recorded as exposed sends one `disexposure` (records only) and is
    /// recorded as not exposed, and without it nothing is sent and the
    /// record is kept, so a resume that finds the element still visible
    /// sends nothing for it. Resuming first lets go of the registrations
    /// whose element was freed while stopped — one recorded as exposed sends
    /// its `disexposure`, as a free while running does — and then arms every
    /// other from its attributes as they are now.
    ///
    /// Registrations are visited in `NodeId` order, so the records one call
    /// sends, and the observers a resume creates, come in an order that does
    /// not depend on the hash map's.
    pub(crate) fn switch(&mut self, document: &mut LynxDocument, on: bool, send_event: bool) {
        if on == self.running {
            return;
        }
        self.running = on;
        let nodes = self.registered();
        if on {
            for node in nodes {
                if document.get(node).is_some() {
                    let listens = self.shared.borrow().entries[&node].listens;
                    let attributes = Attributes::read(document, node, self.ui_margin_default);
                    self.arm(document, node, attributes, listens);
                } else {
                    let mut shared = self.shared.borrow_mut();
                    let Shared {
                        entries,
                        transitions,
                    } = &mut *shared;
                    if let Some(mut entry) = entries.remove(&node) {
                        record(transitions, node, &mut entry, false, false);
                    }
                }
            }
            return;
        }
        let mut observers = Vec::new();
        {
            let mut shared = self.shared.borrow_mut();
            let Shared {
                entries,
                transitions,
            } = &mut *shared;
            for node in nodes {
                let Some(entry) = entries.get_mut(&node) else {
                    continue;
                };
                observers.extend(entry.observer.take());
                if send_event {
                    record(transitions, node, entry, false, false);
                }
            }
        }
        for observer in observers {
            document.drop_intersection_observer(observer);
        }
    }

    /// Every registered element, in `NodeId` order.
    fn registered(&self) -> Vec<NodeId> {
        let mut nodes: Vec<NodeId> = self.shared.borrow().entries.keys().copied().collect();
        nodes.sort_unstable_by_key(|node| node.to_bits());
        nodes
    }

    /// Every pending transition, in the order they formed, leaving none.
    pub(crate) fn take_transitions(&self) -> Vec<Transition> {
        std::mem::take(&mut self.shared.borrow_mut().transitions)
    }

    /// Whether a transition waits for a delivery entry: the epilogue's cue to
    /// post one even when no observer has entries queued, for a teardown or
    /// a `stopExposure` that sent records, or a freed element.
    pub(crate) fn has_transitions(&self) -> bool {
        !self.shared.borrow().transitions.is_empty()
    }

    /// The observer `node`'s registration owns, if it is registered and
    /// detection runs — for the tests that read what an arm created.
    #[cfg(test)]
    pub(crate) fn observer_of(&self, node: NodeId) -> Option<IntersectionObserverId> {
        self.shared
            .borrow()
            .entries
            .get(&node)
            .and_then(|entry| entry.observer)
    }

    /// Whether `node` is registered at all.
    #[cfg(test)]
    fn is_registered(&self, node: NodeId) -> bool {
        self.shared.borrow().entries.contains_key(&node)
    }
}

#[cfg(test)]
#[path = "exposure_tests.rs"]
mod tests;
