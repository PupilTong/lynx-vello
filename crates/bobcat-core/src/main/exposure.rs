//! Lynx exposure (曝光): which elements a page registered for exposure are on
//! screen, told to script as an element's `uiappear`/`uidisappear` and as its
//! `bobcat:exposure` record event, out of which the realm's Lynx layer builds
//! the global `exposure`/`disexposure` record lists.
//!
//! Exposure is a Lynx-only extension (standards bucket 2): no W3C feature has
//! its attributes, its events or its records. It lives here rather than in
//! `terminus`: each registered element is one intersection observer of `terminus`'s
//! (`terminus::link::IntersectionEventHandler`), the W3C primitive with the target
//! margin `terminus` adds beside the root margin, and this module turns what
//! those observers deliver into exposure changes.
//!
//! # The rulings it is built on (user, 2026-10-11; the margin fold is the
//! architect's decision of 2026-10-12)
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
//! **Margins have their native meaning: a positive margin grows the
//! element's own rect, a negative screen margin shrinks the screen.**
//! Each observer has two margins, the root margin that dilates the viewport
//! and the target margin that dilates the element's border box before any
//! clip, and the element's margins are folded into them by one rule:
//! - `exposure-ui-margin-*` lands on the target margin of its own side, either sign, and always
//!   applies, as web-core's does. Native applies it only under an element's
//!   `enable-exposure-ui-margin` or the page's `enableExposureUIMargin`
//!   (`LynxBaseUI.java:1583-1591`); neither is read here. Native grows the element's rect by the
//!   margin the same way (`LynxUIExposure.m:266-334` `frameOfUIInWindow:`,
//!   `UIExposure.java:262-296` `getUIRect`).
//! - A positive `exposure-screen-margin-*` lands on the target margin of the opposite side: growing
//!   the screen down by `b` is growing the element up by `b`. That is native's logic without the
//!   switch, in the same two functions.
//! - A negative `exposure-screen-margin-*` lands on the root margin of its own side, shrinking the
//!   screen there (`LynxUIExposure.m:336-366` `borderOfExposureScreen:`).
//!
//! The target margin is what lets a positive margin reach an element that a
//! scrollport or a clipping page cuts off, as native's per-rect tests do:
//! the primitive clips the grown rect, while a root margin dilates only the
//! viewport, after every clip has cut the element. web-core folds every margin into
//! one `rootMargin` of the nearest scroll container, with signs that cancel
//! a ui margin rather than add it (`ExposureServices.ts:141-151`).
//!
//! **`stopExposure`/`resumeExposure` are native's** (`LynxUIExposure.m:538-569`,
//! `UIExposure.java:478-505`), **for the elements the caller lists** (user
//! rulings 2026-10-16 and 2026-10-20: per card, as both references keep one
//! switch per card, web-core's `createNativeModules.ts:19-30` and native's
//! `LynxExposureModule.java:22-47`). Which elements are a card's is its
//! realm's knowledge — it filed their record listeners — so a switch names
//! its elements, and each registration here carries one flag of its own,
//! stopped or not; nothing here names a realm, a frame or a tree. Stopping an
//! element stops detecting it, so it queues nothing and fires no `uiappear`;
//! with `sendEvent` (its default) each listed element recorded as exposed
//! gets one `disexposure` record and the record is cleared, without it
//! nothing is sent and the record is kept. A stop with `sendEvent` sends what
//! is recorded as exposed whatever an earlier stop did, as native's sends its
//! exposed set on every call. Resuming observes each listed stopped element
//! again, and whichever is visible then and not recorded as exposed is
//! exposed again, `uiappear` included. An element listed by no switch is
//! left alone, so one card's stop leaves the page and every other card
//! running. web-core's switch only gates its global records and keeps
//! observing (`ExposureServices.ts:278-302`).
//!
//! **There is no timer.** Native polls on a display link and web-core batches
//! its records behind a 50 ms timeout (`ExposureServices.ts:246-275`). Here
//! the changes are delivered by the page's intersection delivery entry,
//! the one the epilogue posts after an update that queued entries
//! (`docs/runtime-architecture.md`): first each `uiappear`/`uidisappear`, in
//! the order the changes formed, then each record event, in the same order,
//! followed by the records those listeners queued, all inside the one entry,
//! so the realm's Lynx layer sends one `exposure` list and then one
//! `disexposure` list, web-core's order.
//! `lynx.setObserverFrameRate` has no rate to set.
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
//! - **A freed element sends nothing**, as web-core's does: a record is an event dispatched at its
//!   element, and a freed element is no target. Native reports one that was exposed on its next
//!   pass. An element that is unlinked but alive leaves through the primitive: it is no longer
//!   rendered, so its update reports it not intersecting, and its record is dispatched at it as at
//!   any element.
//! - **Teardown sends a record and no element event.** Unregistering an exposed element, or
//!   changing its id, sends a `disexposure` record carrying the id it was exposed under and fires
//!   no `uidisappear`, as web-core's `sendAppearEvent = false` does (`ExposureServices.ts:86-104`);
//!   so does `stopExposure` with `sendEvent`. A resume that finds an element visible fires
//!   `uiappear` as well as the record, as native does, because resuming is an ordinary update here.
//! - **The primitive's edge cases stand**: a zero-area element that is intersecting has ratio 1, an
//!   element that only touches an edge is intersecting, and `visibility: hidden` or `opacity: 0`
//!   hides nothing from it — where native reports a zero-sized element hidden
//!   (`LynxUIExposure.m:373-375`), needs strict overlap and walks `isVisible`.
//! - **Lengths**: `<n>px`, `<n>rpx` (1rpx is the viewport width over 750, stylo's rule), and a bare
//!   number as px (`UnitUtils.java:59-86`). `<n>%` is of the rect the margin lands on, resolved by
//!   the primitive at every update, top and bottom of its height, left and right of its width: a ui
//!   margin's of the element's own size, native's `viewSize`; a positive screen margin's of the
//!   element's size too, and a negative one's of the viewport. Native has no screen `%`. A
//!   percentage and a length that land on one side together, a ui margin and the opposite positive
//!   screen margin, are not one margin length; the length is kept and the percentage counts as 0.
//!   Anything else is 0, as web-core's `convertLengthToPx` answers for what it cannot read. A
//!   negative length is kept.
//! - **Positive screen margins stay on the element with the ui switch on.** Native then puts every
//!   screen margin on the screen rect, a positive one included (`borderOfExposureScreen:`), and the
//!   element keeps only its ui margins. Here a positive screen margin grows the element in both
//!   switch states. At `exposure-area` 0 the answer is the same, intersecting or not; above it the
//!   ratio is over the grown element rather than the element. A root margin cannot reach past the
//!   page's clip or a scroller's clip, which native's independent per-rect tests do reach, and the
//!   user's 2026-10-11 ruling keeps the clip-through geometry.
//! - **A margin that shrinks the element past its size**, a negative ui margin, leaves a zero-size
//!   rect at the shifted origin, and the primitive's zero-area rule stands: intersecting, it has
//!   ratio 1. Native says such an element is not in the window (`LynxUIExposure.m:315-317`,
//!   `UIExposure.java:282-284`). The root margin's clamp is the same rule.
//! - **`exposure-area`** is `parseFloat(value) / 100`, web-core's (`ExposureServices.ts:107-108`):
//!   `"50%"` and `"50"` are both one half. It is clamped to `[0, 1]`, and what does not parse is 0.
//! - **`exposure-scene`** is `""` when absent.
//!
//! # How a registration is held
//!
//! An element is registered while some realm has filed the exposure record
//! listener on it — a `bobcat:exposure` row in the document's listener
//! table, which the realm's Lynx layer files while the element has a
//! non-empty `exposure-id` or a `uiappear`/`uidisappear` handler — and it
//! hears its element events while the table also has a `uiappear` or
//! `uidisappear` row on it. The host tells this module on each change of
//! either answer, as the realm reports a registration
//! (`listenerOpened`/`listenerClosed`); no attribute registers an element by
//! itself. Each registration owns one observer — root the viewport, one threshold, the
//! element's `exposure-area`, and the root and target margins its margins
//! fold into — whose handler, [`ExposureHandler`], compares what the
//! primitive reports with what was last recorded and queues a
//! [`ExposureChange`] where they differ. One
//! threshold at the area is enough: the primitive queues an entry exactly
//! when the pair (threshold index, intersecting) changes, which is exactly
//! when "intersecting with a ratio of at least the area" can change.
//!
//! Every attribute change re-arms a registered element: the observer is
//! dropped and created again from the attributes as they are now. The first update after an observe
//! always reports the element, so an element whose visibility did not move
//! reports what was already recorded and queues nothing.
//!
//! A registration keeps its observer while it is stopped: the handler
//! ignores what the primitive reports for a stopped element, and a resume
//! re-arms it, so the fresh observer's first report is judged against the
//! record again.
//!
//! The handler is bound to its element, so the primitive drops it when the
//! element is freed; that drop is how a freed element is noticed, and it
//! only lets the registration go, whether or not it is stopped. Every drop
//! this module makes itself clears the registration's observer first, so the
//! handler's `Drop` tells the two apart. That `Drop` borrows the shared state
//! and runs inside `Document::drop_intersection_observer` and inside an
//! element's free (`Document::drop_element`), so no borrow of the shared
//! state is ever held across a `Document` call.

use std::cell::RefCell;
use std::rc::Rc;

use rustc_hash::FxHashMap;
use terminus::link::{
    IntersectionEventHandler, IntersectionMargin, IntersectionObserverEntry,
    IntersectionObserverId, IntersectionObserverInit, MarginLength, NodeId,
};

use super::tree::LynxDocument;

/// The attribute whose presence, non-empty, asks for global records.
const EXPOSURE_ID: &str = "exposure-id";
/// What a record names besides the id; `""` when absent.
const EXPOSURE_SCENE: &str = "exposure-scene";
/// The visible share an element needs, as `parseFloat(value) / 100`.
const EXPOSURE_AREA: &str = "exposure-area";
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
/// registration: every `exposure-` attribute.
///
/// A prefix test rather than the list, because it is asked on every
/// attribute write and a name that matches without meaning anything only
/// costs a re-arm of an element that is registered, and nothing for one that
/// is not.
pub(crate) fn is_exposure_attribute(name: &str) -> bool {
    name.starts_with("exposure-")
}

/// One element's exposure moving: it became visible (`appeared`) or stopped
/// being so, or a teardown took it out of the exposed record.
///
/// The id and the scene are the registration's own, shared rather than
/// copied when the change forms: the element may be freed, or its
/// attributes rewritten, before the delivery entry runs, and a rewrite
/// replaces the registration's strings rather than editing them, so a
/// `disexposure` still names what the element was exposed under.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ExposureChange {
    pub(crate) node: NodeId,
    pub(crate) appeared: bool,
    /// The `exposure-id` this change is recorded under: `None` for an
    /// element registered only by its listener, which gets no record.
    pub(crate) exposure_id: Option<Rc<str>>,
    /// The `exposure-scene`: `None` when absent or empty, which the record
    /// carries as `""`.
    pub(crate) scene: Option<Rc<str>>,
    /// Whether the element hears `uiappear`/`uidisappear` for it: false for
    /// a teardown and for `stopExposure`, which send records only.
    pub(crate) element_event: bool,
}

/// One registered element.
#[derive(Debug)]
struct Entry {
    /// The observer this registration owns. `None` before the first arm,
    /// and cleared before every drop this module makes, which is what
    /// [`ExposureHandler`]'s `Drop` reads.
    observer: Option<IntersectionObserverId>,
    /// The element hears `uiappear`/`uidisappear`: some realm has filed a
    /// handler for one of them on it.
    element_event: bool,
    /// `exposure-id` when present and non-empty, copied so that a freed
    /// element still names it, and shared with every change it forms.
    exposure_id: Option<Rc<str>>,
    /// `exposure-scene` when present and non-empty, the same way.
    scene: Option<Rc<str>>,
    /// `exposure-area` as a ratio in `[0, 1]`.
    area: f64,
    /// Recorded as exposed: the last change this registration queued was an
    /// appearance.
    exposed: bool,
    /// Between a switch that stopped it and one that resumes it
    /// ([`Exposure::switch`]): its observer stays, and the handler ignores
    /// what it reports.
    stopped: bool,
}

/// What [`Exposure`] and every [`ExposureHandler`] share.
#[derive(Debug, Default)]
struct Shared {
    entries: FxHashMap<NodeId, Entry>,
    /// Pending, in the order they formed.
    changes: Vec<ExposureChange>,
}

/// Queues `change` unless it can say nothing: one with no id and no
/// element event would deliver neither a record nor an event, and queued it
/// would still cost a delivery entry.
fn queue(changes: &mut Vec<ExposureChange>, change: ExposureChange) {
    if change.exposure_id.is_some() || change.element_event {
        changes.push(change);
    }
}

/// Records `entry` as exposed, queueing the appearance, unless it already
/// is. `element_event` is whether the element hears it.
fn expose(changes: &mut Vec<ExposureChange>, node: NodeId, entry: &mut Entry, element_event: bool) {
    if entry.exposed {
        return;
    }
    entry.exposed = true;
    queue(changes, change(node, entry, true, element_event));
}

/// Records `entry` as not exposed, queueing the disappearance, if it was
/// exposed; answers whether that queued a change. `element_event` is whether
/// the element hears it.
fn disexpose(
    changes: &mut Vec<ExposureChange>,
    node: NodeId,
    entry: &mut Entry,
    element_event: bool,
) -> bool {
    if !std::mem::take(&mut entry.exposed) {
        return false;
    }
    let before = changes.len();
    queue(changes, change(node, entry, false, element_event));
    changes.len() > before
}

/// `entry`'s change, with the id and scene it carries now.
fn change(node: NodeId, entry: &Entry, appeared: bool, element_event: bool) -> ExposureChange {
    ExposureChange {
        node,
        appeared,
        exposure_id: entry.exposure_id.clone(),
        scene: entry.scene.clone(),
        element_event,
    }
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
    /// Compares each entry with the recorded state and queues a change
    /// where they differ, unless the registration is stopped. Touches no
    /// `Document` method, which is what lets it hold the shared borrow for
    /// the whole call.
    fn notify(
        &mut self,
        _document: &mut LynxDocument,
        observer: IntersectionObserverId,
        entries: Vec<IntersectionObserverEntry>,
    ) {
        let mut shared = self.shared.borrow_mut();
        let Shared {
            entries: registered,
            changes,
        } = &mut *shared;
        let Some(entry) = registered.get_mut(&self.node) else {
            return;
        };
        // Only the registration's current observer speaks for it; a dropped
        // one cannot deliver, so this is a statement of the invariant more
        // than a filter. A stopped element is not detected: what the
        // primitive reports is ignored, and the resume re-arms it.
        if entry.observer != Some(observer) || entry.stopped {
            return;
        }
        for observed in entries {
            // A ratio is never negative, so an area of 0 asks for nothing
            // but intersection.
            let visible = observed.is_intersecting && observed.intersection_ratio >= entry.area;
            let element_event = entry.element_event;
            if visible {
                expose(changes, self.node, entry, element_event);
            } else {
                disexpose(changes, self.node, entry, element_event);
            }
        }
    }

    fn bound_to(&self) -> Option<NodeId> {
        Some(self.node)
    }
}

impl Drop for ExposureHandler {
    /// The element was freed, when its registration still names an
    /// observer: every drop this module makes clears that first. The
    /// registration goes, and nothing is queued, whether or not it is
    /// stopped: a freed element is no target, so there is nothing to
    /// dispatch a record at.
    fn drop(&mut self) {
        let mut shared = self.shared.borrow_mut();
        let freed = shared
            .entries
            .get(&self.node)
            .is_some_and(|entry| entry.observer.is_some());
        if freed {
            shared.entries.remove(&self.node);
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

impl Length {
    /// Whether the length moves no edge, in either unit.
    fn is_zero(self) -> bool {
        match self {
            Self::Px(number) | Self::Percent(number) => number == 0.0,
        }
    }

    /// Whether the length moves an edge out: a positive number in either
    /// unit.
    fn is_positive(self) -> bool {
        match self {
            Self::Px(number) | Self::Percent(number) => number > 0.0,
        }
    }

    /// Two lengths that land on one side of one margin.
    ///
    /// Two lengths of one unit add. A percentage and a length together are
    /// not one [`MarginLength`], and the element's size they would meet in
    /// is only known at each update, so then the length is kept and the
    /// percentage counts as 0.
    fn plus(self, other: Self) -> Self {
        if self.is_zero() {
            return other;
        }
        if other.is_zero() {
            return self;
        }
        match (self, other) {
            (Self::Px(a), Self::Px(b)) => Self::Px(a + b),
            (Self::Percent(a), Self::Percent(b)) => Self::Percent(a + b),
            (Self::Px(px), Self::Percent(_)) | (Self::Percent(_), Self::Px(px)) => Self::Px(px),
        }
    }

    const fn margin(self) -> MarginLength {
        match self {
            Self::Px(px) => MarginLength::Px(px),
            Self::Percent(percent) => MarginLength::Percent(percent),
        }
    }
}

/// The two margins an element's exposure margins fold into: the observer's
/// root margin, which dilates the viewport, and its target margin, which
/// dilates the element's own border box before any clip.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Margins {
    root: IntersectionMargin,
    target: IntersectionMargin,
}

/// Folds an element's margins, each array top, right, bottom, left, into
/// the observer's two. `ui` is all zero when ui margins do not apply.
///
/// - A ui margin lands on the target margin of its own side, either sign.
/// - A positive screen margin lands on the target margin of the opposite side: growing the screen
///   down by `b` is growing the element up by `b`, native's `frameOfUIInWindow:` without the
///   switch.
/// - A negative screen margin lands on the root margin of its own side, native's
///   `borderOfExposureScreen:`.
///
/// A percentage is of the rect it lands on, which the primitive resolves at
/// every update: on the target, of the element's own height (top, bottom)
/// or width (left, right); on the root, of the viewport's.
fn fold_margins(screen: [Length; 4], ui: [Length; 4]) -> Margins {
    let mut root = [Length::Px(0.0); 4];
    let mut target = ui;
    for (side, length) in screen.into_iter().enumerate() {
        if length.is_positive() {
            // Top (0) and bottom (2) face each other, as right (1) and left
            // (3) do.
            let opposite = (side + 2) % 4;
            target[opposite] = target[opposite].plus(length);
        } else if !length.is_zero() {
            root[side] = length;
        }
    }
    let margin = |[top, right, bottom, left]: [Length; 4]| IntersectionMargin {
        top: top.margin(),
        right: right.margin(),
        bottom: bottom.margin(),
        left: left.margin(),
    };
    Margins {
        root: margin(root),
        target: margin(target),
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
    exposure_id: Option<Rc<str>>,
    scene: Option<Rc<str>>,
    area: f64,
    margins: Margins,
}

impl Attributes {
    /// Reads `node`'s exposure attributes.
    fn read(document: &LynxDocument, node: NodeId) -> Self {
        let element = document
            .get(node)
            .expect("an element is registered for exposure only while it is live");
        let width = document.viewport_size().width;
        let length = |name: &str| parse_length(element.attribute(name).as_deref(), width);
        Self {
            exposure_id: element
                .attribute(EXPOSURE_ID)
                .filter(|id| !id.is_empty())
                .map(|id| Rc::from(&*id)),
            scene: element
                .attribute(EXPOSURE_SCENE)
                .filter(|scene| !scene.is_empty())
                .map(|scene| Rc::from(&*scene)),
            area: parse_area(element.attribute(EXPOSURE_AREA).as_deref()),
            margins: fold_margins(SCREEN_MARGINS.map(length), UI_MARGINS.map(length)),
        }
    }
}

/// A document's exposure registrations, each stopped or not, and the
/// changes waiting for a delivery entry.
///
/// Owned by the realm's document slot beside the document.
#[derive(Debug)]
pub(crate) struct Exposure {
    /// Shared with every registration's [`ExposureHandler`].
    shared: Rc<RefCell<Shared>>,
}

impl Exposure {
    /// No registrations.
    pub(crate) fn new() -> Self {
        Self {
            shared: Rc::default(),
        }
    }

    /// An `exposure-*` attribute of the live element `node` was written or
    /// removed: a registered element is re-armed from its attributes as they
    /// are now. An element that is not registered stays so — no attribute
    /// registers one by itself.
    pub(crate) fn attribute_changed(&mut self, document: &mut LynxDocument, node: NodeId) {
        let element_event = self
            .shared
            .borrow()
            .entries
            .get(&node)
            .map(|entry| entry.element_event);
        if let Some(element_event) = element_event {
            let attributes = Attributes::read(document, node);
            self.arm(document, node, attributes, element_event);
        }
    }

    /// What the document's listener table says of the live element `node`,
    /// told on each change of either answer: `registered`, whether some realm
    /// has filed the exposure record listener on it, and `element_event`,
    /// whether some realm has filed a `uiappear`/`uidisappear` handler on it.
    ///
    /// The element is registered while `registered` holds: the first time,
    /// it is armed from its attributes; after that, a change of
    /// `element_event` alone only changes what its next changes say. When
    /// `registered` stops holding, it is torn down.
    pub(crate) fn set_registered(
        &mut self,
        document: &mut LynxDocument,
        node: NodeId,
        registered: bool,
        element_event: bool,
    ) {
        if !registered {
            self.teardown(document, node);
            return;
        }
        if let Some(entry) = self.shared.borrow_mut().entries.get_mut(&node) {
            entry.element_event = element_event;
            return;
        }
        let attributes = Attributes::read(document, node);
        self.arm(document, node, attributes, element_event);
    }

    /// Registers `node`, or re-arms it, with `attributes`.
    ///
    /// An existing observer is dropped. A registration recorded as exposed
    /// under another id than the one it now has sends a `disexposure` under
    /// the old id and is recorded as not exposed, so the new id gets an
    /// exposure of its own. A fresh observer is created and observes the
    /// element, whether or not the registration is stopped: the handler
    /// ignores what it reports while it is.
    fn arm(
        &mut self,
        document: &mut LynxDocument,
        node: NodeId,
        attributes: Attributes,
        element_event: bool,
    ) {
        let Attributes {
            exposure_id,
            scene,
            area,
            margins,
        } = attributes;
        let previous = {
            let mut shared = self.shared.borrow_mut();
            let Shared { entries, changes } = &mut *shared;
            let entry = entries.entry(node).or_insert(Entry {
                observer: None,
                element_event,
                exposure_id: None,
                scene: None,
                area: 0.0,
                exposed: false,
                stopped: false,
            });
            let previous = entry.observer.take();
            if entry.exposure_id != exposure_id {
                disexpose(changes, node, entry, false);
            }
            entry.element_event = element_event;
            entry.exposure_id = exposure_id;
            entry.scene = scene;
            entry.area = area;
            previous
        };
        if let Some(observer) = previous {
            document.drop_intersection_observer(observer);
        }
        let observer = document.create_intersection_observer(
            Box::new(ExposureHandler {
                node,
                shared: Rc::clone(&self.shared),
            }),
            IntersectionObserverInit {
                root: None,
                root_margin: margins.root,
                target_margin: margins.target,
                thresholds: vec![area],
            },
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
            let Shared { entries, changes } = &mut *shared;
            let Some(mut entry) = entries.remove(&node) else {
                return;
            };
            disexpose(changes, node, &mut entry, false);
            entry.observer
        };
        if let Some(observer) = observer {
            document.drop_intersection_observer(observer);
        }
    }

    /// `lynx.stopExposure({sendEvent})` (`on` false) and
    /// `lynx.resumeExposure()` (`on` true) for the elements `nodes`, the ones
    /// the calling realm registered, as native runs them for one card.
    /// Answers how many records the call queued: one per listed element
    /// recorded as exposed under an `exposure-id` for a stop with
    /// `send_event`, and 0 for anything else.
    ///
    /// Stopping marks each listed registration stopped: its observer stays,
    /// and what it reports is ignored. With `send_event` each listed
    /// registration recorded as exposed sends one `disexposure` (records
    /// only) and is recorded as not exposed, whatever an earlier stop did;
    /// without it nothing is sent and the record is kept, so a resume that
    /// finds the element still visible sends nothing for it. Resuming clears
    /// the mark of each listed stopped registration and arms it again from
    /// its attributes as they are now, so its fresh observer's first report
    /// is judged against the record: one whose element did not move queues
    /// nothing. A listed registration that is not stopped is running already
    /// and is left alone.
    ///
    /// A listed node that is not registered — never, no longer, or freed,
    /// which lets its registration go with its handler, sending nothing — is
    /// skipped. Registrations are visited in the order listed, so the records
    /// one call sends come in the caller's order.
    pub(crate) fn switch(
        &mut self,
        document: &mut LynxDocument,
        on: bool,
        send_event: bool,
        nodes: &[NodeId],
    ) -> usize {
        if !on {
            let mut shared = self.shared.borrow_mut();
            let Shared { entries, changes } = &mut *shared;
            let mut queued = 0;
            for &node in nodes {
                let Some(entry) = entries.get_mut(&node) else {
                    continue;
                };
                entry.stopped = true;
                if send_event && disexpose(changes, node, entry, false) {
                    queued += 1;
                }
            }
            return queued;
        }
        for &node in nodes {
            let element_event = match self.shared.borrow_mut().entries.get_mut(&node) {
                Some(entry) if entry.stopped => {
                    entry.stopped = false;
                    entry.element_event
                }
                _ => continue,
            };
            let attributes = Attributes::read(document, node);
            self.arm(document, node, attributes, element_event);
        }
        0
    }

    /// Every pending change, in the order they formed, leaving none.
    pub(crate) fn take_changes(&self) -> Vec<ExposureChange> {
        std::mem::take(&mut self.shared.borrow_mut().changes)
    }

    /// Whether a change waits for a delivery entry: the epilogue's cue to
    /// post one even when no observer has entries queued, for a teardown or
    /// a `stopExposure` that sent records.
    pub(crate) fn has_changes(&self) -> bool {
        !self.shared.borrow().changes.is_empty()
    }

    /// The observer `node`'s registration owns, if it is registered — for
    /// the tests that read what an arm created.
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

    /// Whether `node` is registered and recorded as exposed.
    #[cfg(test)]
    fn is_exposed(&self, node: NodeId) -> bool {
        self.shared
            .borrow()
            .entries
            .get(&node)
            .is_some_and(|entry| entry.exposed)
    }

    /// Whether `node` is registered and stopped.
    #[cfg(test)]
    fn is_stopped(&self, node: NodeId) -> bool {
        self.shared
            .borrow()
            .entries
            .get(&node)
            .is_some_and(|entry| entry.stopped)
    }
}

#[cfg(test)]
#[path = "exposure_tests.rs"]
mod tests;
