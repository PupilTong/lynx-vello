//! Lynx exposure (曝光): which elements a page registered for exposure are on
//! screen, told to script as an element's `uiappear`/`uidisappear` and as the
//! global `exposure`/`disexposure` record lists.
//!
//! Exposure is a Lynx-only extension (standards bucket 2): no W3C feature has
//! its attributes, its events or its records. It lives here rather than in
//! `dom`: each registered element is one intersection observer of `dom`'s
//! (`dom::IntersectionEventHandler`), the W3C primitive with the target
//! margin `dom` adds beside the root margin, and this module turns what
//! those observers deliver into exposure transitions.
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
//! `UIExposure.java:478-505`), **scoped by shadow host** (user ruling
//! 2026-10-16: per card, as both references keep one switch per card,
//! web-core's `createNativeModules.ts:19-30` and native's
//! `LynxExposureModule.java:22-47`). A switch names the shadow host of the
//! tree it stops — `None` for the document tree, a `frame` for the card that
//! renders in it — and reaches the elements whose innermost shadow host that
//! is: an element recorded as exposed by the host it was exposed under, any
//! other by the tree it is in when the primitive reports it, so an element
//! registered while detached is judged when it appears. Stopping a scope
//! stops detecting its elements, so they queue nothing and fire no
//! `uiappear`; with `sendEvent` (its default) every element of the scope
//! recorded as exposed gets one `disexposure` record and the record is
//! cleared, without it nothing is sent and the record is kept. Resuming
//! observes every registered element again, registrations made while stopped
//! included, and whatever is visible then and not recorded as exposed is
//! exposed again, `uiappear` included. Each scope is its own: stopping the
//! document tree leaves every card's elements detected, and stopping one
//! card leaves the page and every other card alone, its own cards included.
//! web-core's switch only gates its global records and keeps observing
//! (`ExposureServices.ts:278-302`).
//!
//! **Each exposure carries the shadow hosts above its element** (its
//! [`HostChain`], innermost first): empty for the document tree, `[F]` for an
//! element of the card the frame `F` shows, `[G, F]` for one of the card the
//! frame `G` inside that card shows. It is read when the element is exposed
//! and kept with the record, as the id and the scene are, so the
//! `disexposure` of an element that was since freed, moved or detached still
//! names where it was exposed. Which realm hears a record is the realms' own
//! rule over that list (`bobcat-element`'s `element-papi.ts`): nothing here
//! names a realm or a frame.
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
//!   an ordinary update here. An element freed while its scope is stopped and its record kept sends
//!   its `disexposure` when that scope resumes, as native's next pass after a resume does.
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
//! An element is registered while it has a non-empty `exposure-id` or the
//! realm has filed a `uiappear`/`uidisappear` handler on it, which the
//! realm reports through the `exposureEvents` host member. Each
//! registration owns one observer — root the viewport, one threshold, the
//! element's `exposure-area`, and the root and target margins its margins
//! fold into — whose handler, [`ExposureHandler`], compares what the
//! primitive reports with what was last recorded and queues a
//! [`Transition`] where they differ. One
//! threshold at the area is enough: the primitive queues an entry exactly
//! when the pair (threshold index, intersecting) changes, which is exactly
//! when "intersecting with a ratio of at least the area" can change.
//!
//! Every attribute change re-arms: the observer is dropped and created again
//! from the attributes as they are now. The first update after an observe
//! always reports the element, so an element whose visibility did not move
//! reports what was already recorded and queues nothing.
//!
//! A registration keeps its observer while its scope is stopped: the handler
//! ignores what the primitive reports for an element of a stopped scope, and
//! a resume re-arms every registration, so the fresh observer's first report
//! is judged against the record again.
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
    IntersectionEventHandler, IntersectionMargin, IntersectionObserverEntry,
    IntersectionObserverId, IntersectionObserverInit, MarginLength, NodeId,
};
use rustc_hash::FxHashMap;

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

/// The shadow hosts above an element, innermost first: empty for an element
/// of the document tree, `[F]` for one in the shadow tree `F` hosts, `[G, F]`
/// for one in the shadow tree `G` hosts, `G` being in `F`'s. Read when the
/// element is exposed, and kept with the record.
pub(crate) type HostChain = Vec<NodeId>;

/// The shadow host of the tree an element is in, `None` for the document
/// tree: the scope `stopExposure`/`resumeExposure` reach the element in.
pub(crate) type Scope = Option<NodeId>;

/// `node`'s [`HostChain`] as the tree stands: every shadow host on its
/// shadow-including ancestor chain, innermost first. A detached tree ends
/// the walk as the document tree does.
fn host_chain(document: &LynxDocument, node: NodeId) -> HostChain {
    let mut chain = HostChain::new();
    let mut at = node;
    while let Some(host) = innermost_host(document, at) {
        chain.push(host);
        at = host;
    }
    chain
}

/// The shadow host of the tree `node` is in, as the tree stands.
fn innermost_host(document: &LynxDocument, node: NodeId) -> Scope {
    document
        .shadow_root_of(node)
        .filter(|root| *root != node)
        .and_then(|root| document.shadow_host(root))
}

/// One element's exposure moving: it became visible (`appeared`) or stopped
/// being so, or a teardown took it out of the exposed record.
///
/// The id, the scene and the hosts are copied when the transition forms,
/// because the element may be freed, moved, or its attributes rewritten,
/// before the delivery entry runs: a `disexposure` names what, and where, the
/// element was exposed under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Transition {
    pub(crate) node: NodeId,
    pub(crate) appeared: bool,
    /// The `exposure-id` this transition is recorded under: `None` for an
    /// element registered only by its listener, which gets no record.
    pub(crate) exposure_id: Option<String>,
    pub(crate) scene: String,
    /// The shadow hosts above the element when it was exposed, innermost
    /// first; see [`HostChain`].
    pub(crate) hosts: HostChain,
    /// Whether the element hears `uiappear`/`uidisappear` for it: false for
    /// a teardown and for `stopExposure`, which send records only.
    pub(crate) element_event: bool,
}

/// One registered element.
#[derive(Debug)]
struct Entry {
    /// The observer this registration owns. `None` before the first arm and
    /// for an element freed while its scope was stopped and its record kept,
    /// and cleared before every drop this module makes, which is what
    /// [`ExposureHandler`]'s `Drop` reads.
    observer: Option<IntersectionObserverId>,
    /// The realm has filed a `uiappear`/`uidisappear` handler on it.
    listens: bool,
    /// `exposure-id` when present and non-empty, copied so that a freed
    /// element still names it.
    exposure_id: Option<String>,
    scene: String,
    /// `exposure-area` as a ratio in `[0, 1]`.
    area: f64,
    /// Recorded as exposed — the last transition this registration queued
    /// was an appearance — with the hosts above the element then.
    exposed: Option<HostChain>,
}

impl Entry {
    /// The scope that reaches the element: the host it was exposed under
    /// while it is recorded as exposed, the tree it is in now otherwise.
    fn scope(&self, document: &LynxDocument, node: NodeId) -> Scope {
        match &self.exposed {
            Some(hosts) => hosts.first().copied(),
            None => innermost_host(document, node),
        }
    }

    /// The scope the element was exposed under, when it is recorded as
    /// exposed.
    fn exposed_scope(&self) -> Option<Scope> {
        self.exposed.as_ref().map(|hosts| hosts.first().copied())
    }
}

/// What [`Exposure`] and every [`ExposureHandler`] share.
#[derive(Debug, Default)]
struct Shared {
    entries: FxHashMap<NodeId, Entry>,
    /// Pending, in the order they formed.
    transitions: Vec<Transition>,
    /// The scopes between a `stopExposure` and its `resumeExposure`: a
    /// handful at most, one per card that stopped.
    stopped: Vec<Scope>,
}

/// Queues `transition` unless it can say nothing: one with no id and no
/// element event would deliver neither a record nor an event, and queued it
/// would still cost a delivery entry.
fn queue(transitions: &mut Vec<Transition>, transition: Transition) {
    if transition.exposure_id.is_some() || transition.element_event {
        transitions.push(transition);
    }
}

/// Records `entry` as exposed under `hosts`, queueing the appearance, unless
/// it already is. `element_event` is whether the element hears it.
fn expose(
    transitions: &mut Vec<Transition>,
    node: NodeId,
    entry: &mut Entry,
    hosts: HostChain,
    element_event: bool,
) {
    if entry.exposed.is_some() {
        return;
    }
    entry.exposed = Some(hosts.clone());
    queue(
        transitions,
        Transition {
            node,
            appeared: true,
            exposure_id: entry.exposure_id.clone(),
            scene: entry.scene.clone(),
            hosts,
            element_event,
        },
    );
}

/// Records `entry` as not exposed, queueing the disappearance under the
/// hosts it was exposed under, if it was exposed. `element_event` is whether
/// the element hears it.
fn disexpose(
    transitions: &mut Vec<Transition>,
    node: NodeId,
    entry: &mut Entry,
    element_event: bool,
) {
    let Some(hosts) = entry.exposed.take() else {
        return;
    };
    queue(
        transitions,
        Transition {
            node,
            appeared: false,
            exposure_id: entry.exposure_id.clone(),
            scene: entry.scene.clone(),
            hosts,
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
    /// where they differ, unless the element's scope is stopped. Only reads
    /// the `Document` — the element's shadow hosts — which is what lets it
    /// hold the shared borrow for the whole call.
    fn notify(
        &mut self,
        document: &mut LynxDocument,
        observer: IntersectionObserverId,
        entries: Vec<IntersectionObserverEntry>,
    ) {
        let mut shared = self.shared.borrow_mut();
        let Shared {
            entries: registered,
            transitions,
            stopped,
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
            // A stopped scope's elements are not detected: what the
            // primitive reports is ignored, and the resume re-arms them.
            if !stopped.is_empty() && stopped.contains(&entry.scope(document, self.node)) {
                continue;
            }
            // A ratio is never negative, so an area of 0 asks for nothing
            // but intersection.
            let visible = observed.is_intersecting && observed.intersection_ratio >= entry.area;
            let element_event = entry.listens;
            if visible {
                if entry.exposed.is_none() {
                    let hosts = host_chain(document, self.node);
                    expose(transitions, self.node, entry, hosts, element_event);
                }
            } else {
                disexpose(transitions, self.node, entry, element_event);
            }
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
    /// goes; unless the scope it was exposed under is stopped, when the
    /// registration is kept, with no observer, for that scope's resume to
    /// let go of.
    fn drop(&mut self) {
        let mut shared = self.shared.borrow_mut();
        let Shared {
            entries,
            transitions,
            stopped,
        } = &mut *shared;
        let Some(entry) = entries.get_mut(&self.node) else {
            return;
        };
        if entry.observer.is_none() {
            return;
        }
        if entry
            .exposed_scope()
            .is_some_and(|scope| stopped.contains(&scope))
        {
            entry.observer = None;
            return;
        }
        if let Some(mut entry) = entries.remove(&self.node) {
            disexpose(transitions, self.node, &mut entry, false);
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
    exposure_id: Option<String>,
    scene: String,
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
        let length = |name: &str| parse_length(element.attribute(name), width);
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
            margins: fold_margins(SCREEN_MARGINS.map(length), UI_MARGINS.map(length)),
        }
    }
}

/// A document's exposure registrations, the scopes `stopExposure` stopped
/// and `resumeExposure` has not resumed, and the transitions waiting for a
/// delivery entry.
///
/// Owned by the realm's document slot beside the document.
#[derive(Debug)]
pub(crate) struct Exposure {
    /// Shared with every registration's [`ExposureHandler`].
    shared: Rc<RefCell<Shared>>,
}

impl Exposure {
    /// No registrations, detection running in every scope.
    pub(crate) fn new() -> Self {
        Self {
            shared: Rc::default(),
        }
    }

    /// An `exposure-*` attribute of the live element `node` was written or
    /// removed: the registration follows the attributes as they are now.
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
        let attributes = Attributes::read(document, node);
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
    /// exposure of its own. A fresh observer is created and observes the
    /// element, whether or not its scope is stopped: the handler ignores what
    /// it reports while it is.
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
            margins,
        } = attributes;
        let previous = {
            let mut shared = self.shared.borrow_mut();
            let Shared {
                entries,
                transitions,
                ..
            } = &mut *shared;
            let entry = entries.entry(node).or_insert(Entry {
                observer: None,
                listens,
                exposure_id: None,
                scene: String::new(),
                area: 0.0,
                exposed: None,
            });
            let previous = entry.observer.take();
            if entry.exposure_id != exposure_id {
                disexpose(transitions, node, entry, false);
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
            let Shared {
                entries,
                transitions,
                ..
            } = &mut *shared;
            let Some(mut entry) = entries.remove(&node) else {
                return;
            };
            disexpose(transitions, node, &mut entry, false);
            entry.observer
        };
        if let Some(observer) = observer {
            document.drop_intersection_observer(observer);
        }
    }

    /// `lynx.stopExposure({sendEvent})` (`on` false) and
    /// `lynx.resumeExposure()` (`on` true) for the elements of `scope`, the
    /// shadow host of the calling card's tree (`None` for the document
    /// tree), as native runs them for one card. Stopping a stopped scope and
    /// resuming a running one do nothing.
    ///
    /// Stopping stops detecting the scope's elements, each judged by
    /// [`Entry::scope`]: their observers stay, and what they report is
    /// ignored. With `send_event` every registration of the scope recorded
    /// as exposed sends one `disexposure` (records only) and is recorded as
    /// not exposed, and without it nothing is sent and the record is kept, so
    /// a resume that finds the element still visible sends nothing for it.
    /// Resuming first lets go of the registrations of the scope whose element
    /// was freed while it was stopped — each was recorded as exposed, and
    /// sends its `disexposure`, as a free while running does — and then arms
    /// every live registration again from its attributes as they are now, so
    /// each fresh observer's first report is judged against the record: one
    /// whose element did not move queues nothing.
    ///
    /// Registrations are visited in `NodeId` order, so the records one call
    /// sends, and the observers a resume creates, come in an order that does
    /// not depend on the hash map's.
    pub(crate) fn switch(
        &mut self,
        document: &mut LynxDocument,
        on: bool,
        send_event: bool,
        scope: Scope,
    ) {
        // Nothing to do — the common case, a card let go of resumes its scope
        // whether or not it stopped it — costs no walk of the registrations.
        {
            let mut shared = self.shared.borrow_mut();
            let stopped = &mut shared.stopped;
            let at = stopped.iter().position(|stopped| *stopped == scope);
            match (on, at) {
                (false, None) => stopped.push(scope),
                (true, Some(at)) => {
                    stopped.swap_remove(at);
                }
                _ => return,
            }
            if !on && !send_event {
                return;
            }
        }
        let nodes = self.registered();
        let mut shared = self.shared.borrow_mut();
        let Shared {
            entries,
            transitions,
            ..
        } = &mut *shared;
        if !on {
            for node in nodes {
                let Some(entry) = entries.get_mut(&node) else {
                    continue;
                };
                if entry.exposed_scope() == Some(scope) {
                    disexpose(transitions, node, entry, false);
                }
            }
            return;
        }
        let mut live = Vec::with_capacity(nodes.len());
        for node in nodes {
            if document.get(node).is_some() {
                live.push(node);
            } else if entries
                .get(&node)
                .is_some_and(|entry| entry.exposed_scope() == Some(scope))
                && let Some(mut entry) = entries.remove(&node)
            {
                disexpose(transitions, node, &mut entry, false);
            }
        }
        drop(shared);
        for node in live {
            let listens = self.shared.borrow().entries[&node].listens;
            let attributes = Attributes::read(document, node);
            self.arm(document, node, attributes, listens);
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

    /// The observer `node`'s registration owns, if it is registered and its
    /// element is live — for the tests that read what an arm created.
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

    /// The hosts `node` is recorded as exposed under, if it is — for the
    /// tests that read what an exposure kept.
    #[cfg(test)]
    fn exposed_hosts(&self, node: NodeId) -> Option<HostChain> {
        self.shared
            .borrow()
            .entries
            .get(&node)
            .and_then(|entry| entry.exposed.clone())
    }
}

#[cfg(test)]
#[path = "exposure_tests.rs"]
mod tests;
