//! The `x-refresh-view`, `x-refresh-header` and `x-refresh-footer` tags: a
//! column whose content can be pulled down past its top to show a header,
//! and pushed up past its end to show a footer, each of which stays shown
//! once it is pulled far enough in.
//!
//! Translated from web-elements'
//! `lynx-stack/packages/web-platform/web-elements/src/elements/XRefreshView/x-refresh-view.css`
//! and its template (`htmlTemplates.ts:174-219`). Only web-core's three tag
//! names exist here. web-core's tag map also turns `<refresh>` and
//! `<refresh-header>` into the first two (`web-core/ts/constants.ts:112-113`);
//! those two spellings are not registered.
//!
//! # The model
//!
//! [`RefreshView`] attaches a UA shadow tree when the element is constructed,
//! web-core's, part for part:
//!
//! - `#container`, the scroll container: a column flex box of the host's size, `overflow-y:
//!   scroll`, `overscroll-behavior: contain`, `scroll-snap-type: y mandatory`.
//! - `#placeholder-top`, an empty box `30%` of `#container`'s height: how far past its own height
//!   the header can be pulled down.
//! - `<slot name="header">`, which the header is assigned to.
//! - `#content`, a column flex box exactly `#container`'s size, holding the default `<slot>` the
//!   content children are assigned to. It is the only box that snaps at rest (`scroll-snap-align:
//!   center`), and `scroll-initial-target: nearest` scrolls `#container` to it on the first layout,
//!   so the view opens with the header above the scrollport and the footer below it.
//! - `<slot name="footer">` and `#placeholder-bot`, the footer's counterparts of the first two.
//!
//! The slots are `display: contents`, so the header, the content children and the footer are
//! flex items of `#container` and `#content` in the flat tree, while the light-DOM rules
//! ([`UA_RULES`]) match them as the host's children. A pull is the scroll chain: a drag on an
//! inner `scroll-view` at its top edge hands what it cannot take to `#container`, the inner
//! scroller's flat-tree scroll parent, and the painter holds every container a drag moves until
//! the release, then glides each to its snap position (`ScrollIntents::settle` in
//! `crates/terminus/src/painter/mod.rs`).
//!
//! # Slot assignment
//!
//! web-core's header and footer set their own `slot` attribute when they connect
//! (`XRefreshHeader.ts`, `XRefreshFooter.ts`). Here [`RefreshView`] does it: on the standard's
//! `slotchange` at its default slot, every `x-refresh-header` assigned there gets `slot="header"`
//! and every `x-refresh-footer` `slot="footer"`. That moves it to its named slot and fires
//! `slotchange` again after the current set; the second pass finds neither tag in the default
//! slot. The header and the footer need no component of their own.
//!
//! # The snap alignment is a view-timeline animation
//!
//! On release web-core's scroller snaps back to `#content` unless the header (footer) is at
//! least 90% visible: then an `IntersectionObserver` has set `x-magnet-enable` on it
//! (`XRefreshSubElementIntersectionObserver.ts`), whose rule gives it `scroll-snap-align: start
//! !important` (`end` for the footer), and the release lands on the header instead. Here the same
//! alignment is an animation of `scroll-snap-align` on the header's own `view(block)` timeline.
//! The property is discrete, so it changes half way between two keyframes:
//!
//! - header: `start` from `cover 0%` to `exit 0%`, `none` from `exit 20%` to `cover 100%`; the
//!   change sits at `exit 10%`, so a header at least 90% visible is `start`. At rest its end edge
//!   is on the scrollport's start (`exit 100%`): `none`.
//! - footer: `none` from `cover 0%` to `entry 80%`, `end` from `entry 100%`; the change sits at
//!   `entry 90%`. At rest its start edge is on the scrollport's end (`entry 0%`): `none`.
//!
//! Neither animates a property the painter samples, so each re-cascades on the main thread when
//! it adopts a scroll of `#container`, and the commit that follows publishes the snap positions
//! the release reads. web-core keeps the attribute until `finishRefresh` removes it; here the
//! alignment follows the header's visibility, so a header that snapped in stays until the user
//! scrolls it out or `finishRefresh` scrolls `#content` back in (`docs/tracking/deviations.md`).
//!
//! # Attributes
//!
//! Attribute values arrive as strings: this engine's `__SetAttribute` stringifies a value
//! (`packages/bobcat-element/src/element-papi.ts`), so `enable-refresh={false}` in JSX is
//! the attribute `"false"`. web-core keeps `"false"` for this component's three attributes
//! (`notToFilterFalseAttributes`, `XRefreshView.ts:19-23`) and matches it in its CSS, as here:
//!
//! - `enable-refresh="false"`: the header generates no box and `#placeholder-top` is hidden, so
//!   nothing can be pulled down. Any other value, or none, shows them.
//! - `enable-loadmore="false"`: the same for the footer and `#placeholder-bot`.
//! - `enable-auto-loadmore="true"` (that value alone, as web-core compares it) also starts a load
//!   when a finger lifts with the footer showing at all (see "Events"); no visible effect.
//! - `enable-footer-rebound` has no effect.
//!
//! # Which headers and footers show
//!
//! The first `x-refresh-header` and the first `x-refresh-footer` among a refresh view's children
//! show; the hide rule matches only what web-core hides — a later one of either, one whose view
//! turns its end off, and one whose parent is some other element. A header with no parent
//! element at all (the top of a shadow tree) is not hidden, an edge accepted here. A shown one
//! takes the engine's display policy, like every container tag ([`super::ua_sheet`]):
//! `display: linear` (a column) under `defaultDisplayLinear`, as web-core's linear toggle gives
//! it and as native's header is a plain group, and the engine default otherwise.
//!
//! # The inner scroller chains on y
//!
//! Every content child gets `overscroll-behavior-y: auto !important`. A y boundary that contains
//! the chain — an author's `overscroll-behavior`, or the `contain` a `scroll-coordinator`
//! declares on itself — would stop the pull at the child, and nothing would reach `#container`.
//! A `scroll-view`'s `bounces` attribute reaches no style at all. Native attaches the header to
//! the inner scroller's own bounce; here the inner scroller does not bounce on y inside a
//! refresh view.
//!
//! # The three UI methods
//!
//! web-core's `XRefreshView` has three methods (`XRefreshView.ts:26-44`, documented at
//! `index.ts:27-29`), none of which reads params, and [`RefreshView`]'s
//! [`CustomElement::invoke`] answers each with code 0, whatever `params` is and on a view with no
//! box too. Each is CSSOM-View's "scroll an element into view"
//! ([`terminus::link::Document::scroll_into_view`]) in the element's nearest scroll container,
//! `#container`: smooth for the two finishing methods, instant for `autoStartRefresh`. A UI method
//! never flushes, so the positions are the last completed layout's.
//!
//! - `finishRefresh` and `finishLoadMore` ([`finish`]) smoothly scroll `#content` back into view at
//!   `block: center`. web-core removes `x-magnet-enable` from the first header (footer), and the
//!   browser re-snaps `#container` to `#content`. `#content` snaps at `center` and is exactly the
//!   scrollport's size, so `center` is its snap position, the one the view opens on, and the
//!   painter's at-rest snap rule leaves it there. As the header (footer) leaves the scrollport its
//!   alignment animation turns it to `none`, so the next release does not snap back to it. A view
//!   already resting on `#content` records no request: the standard call compares the position with
//!   the current one. Both methods scroll whichever end is shown, which is what web-core's re-snap
//!   does too.
//! - `autoStartRefresh` ([`auto_start_refresh`]) scrolls the header into view at `block: start`,
//!   instantly. The header is the first element assigned to the `header` slot (the first
//!   `x-refresh-header` child). web-core sets `x-magnet-enable` on it and smooth-scrolls
//!   `#container` to `top: 0`, which shows the top placeholder above the header, after which the
//!   magnet's `start` snaps the header to the scrollport's start: this call aims at that resting
//!   position directly. A view with no header, or with `enable-refresh="false"` (the header
//!   generates no box), records nothing; web-core's optional chaining skips the attribute there and
//!   its `scroll({top: 0})` still scrolls to the top placeholder.
//!
//! `autoStartRefresh` is instant where web-core's is smooth. The painter glides a smooth request
//! to where the frame's at-rest snap rule would settle its target (`apply_request` in
//! `crates/terminus/src/painter/mod.rs`), and the header is a snap position only once it is at
//! least 90% visible. From rest it is not, so a smooth request to it would be settled back to
//! `#content` and nothing would move. An instant request moves the document at once; the
//! commit that carries it lays out, which re-samples the header's alignment animation at the new
//! offset (`Document::layout` resolves every timeline), so the same frame publishes the header
//! as a snap position and the painter's at-rest rule leaves `#container` on it. A smooth version
//! needs a request the painter does not settle at request time; that is left as a follow-up for
//! the user. The two finishing methods aim at `#content`, which is always a snap position, so
//! their smooth requests are not affected.
//!
//! Each method also moves the component's latches (see "Events"): `finishRefresh` ends the
//! refresh and `finishLoadMore` the load, so the next rest on the header (the footer) starts one
//! again, and `autoStartRefresh` fires `startrefresh` itself.
//!
//! The view keeps a `manual` flag per element, the `isManual` the next `startrefresh` reports:
//! web-core's `_nextRefreshIsManual` (`XRefreshView.ts:24`) and iOS's `manualRefresh`. It is
//! `true` except inside an `autoStartRefresh` call, which clears it, sends its `startrefresh`
//! `{isManual: false}` and sets it again, as iOS's `startRefresh` sets it back after sending
//! (`lynx/platform/darwin/ios/lynx_xelement/refresh/LynxUIRefresh.m:247-262`); web-core sets it
//! back after its next `touchend` sent the event.
//!
//! # Events
//!
//! The component decides seven events from what `#container` and the touches on the view tell it
//! (user ruling 2026-10-20: a Rust component reacting to W3C events, nothing reported by the
//! painter). `#container`'s `scroll` and `scrollend` neither bubble nor are composed, so their
//! path ends at the view's shadow root, which delivers them to the view's
//! [`CustomElement::handle_event`] in the capture pass; the component takes them when their target
//! is `#container` and no other node. Each event is queued at the view — not bubbling —
//! only when a listener on the view registered for its name
//! ([`terminus::link::Document::has_listener`]), a `{}` detail through [`ComponentEvents::queue`]
//! and the others through [`ComponentEvents::queue_detailed`]; the latches are recorded whether or
//! not anything listens. Geometry is the last completed layout's, read at the delivery, and never
//! flushed: the header's and the footer's rectangles in `#container`'s scroll coordinates
//! ([`rect_in_scroller`]; each is a box child of `#container` through its `display: contents` slot)
//! and `#container`'s offset.
//!
//! How far an end shows is native's `pullingPercent`, the length of the end inside the scrollport
//! over its height: `percent = (header.max_y - offset.y) / header.height` — 1 with the header's
//! start on the scrollport's start, 0 with it scrolled out (the view at rest on `#content`), above
//! 1 while the top placeholder shows — and
//! `footer_percent = (offset.y + scrollport.height - footer.min_y) / footer.height`. An end that
//! generates no box (none, a later one, `enable-refresh="false"` / `enable-loadmore="false"`) or
//! has no height fires none of its events, and a view with no box decides nothing.
//!
//! - `headershow` then `headeroffset`, both `{isDragging, offsetPercent}`: each `scroll` with
//!   `percent > 0`, `offsetPercent = percent` (iOS sends both from `didUpdateShowingPrecent`,
//!   `LynxUIRefresh.m:341-364`; web-core too, from one handler).
//! - `footeroffset` `{isDragging, offsetPercent}`: each `scroll` with `footer_percent > 0`.
//! - `headerreleased` `{}`: the `touchend` the view's touch count takes, with `percent >= 1` (iOS:
//!   pulled at least the header's height, `:268-284`, before `startrefresh`).
//! - `footerreleased` `{}`: the same `touchend`, with `footer_percent >= 1`.
//! - `startrefresh` `{isManual}`: the `scrollend` that rests `#container` on the header's snap
//!   position — `percent >= 1` less one CSS px of the header's height ([`LANDING_TOLERANCE`]) — and
//!   `autoStartRefresh`, at once; `isManual` is the `manual` flag above. A refresh latches: no
//!   second `startrefresh` fires until a rest elsewhere or `finishRefresh` ends it.
//! - `startloadmore` `{}`: the `scrollend` that rests `#container` on the footer's snap position,
//!   by the same tolerance, latched the same way (`finishLoadMore` ends it); and under
//!   `enable-auto-loadmore="true"`, the counted `touchend` with `footer_percent > 0` (web-core's
//!   footer-showing rule), which latches, so the rest on the footer that may follow adds nothing.
//!
//! A rest on `#content`, neither end, ends both latches. `isDragging` is whether a finger is down
//! on the view or inside it ([`super::touch_count::TouchCount`]): web-core's touch flag, standing
//! in for native's drag state (user ruling 2026-10-20).
//!
//! # Deviations and what is not implemented
//!
//! - web-core gives `#container`, `#content` and the slot the host's display mode (`--lynx-display:
//!   inherit`, `x-refresh-view.css:22-26`); here they are fixed flex columns.
//! - The header's and the footer's `position: relative !important` are web-core's own and stay
//!   `!important`: web-core's demo card writes `position: absolute` on its header
//!   (`basic-element-x-refresh-view-demo/index.css`). `docs/style-assumptions.md` §D.15 records
//!   them, with the content children's `overscroll-behavior-y`.
//! - The UI methods are the standard scroll-into-view rather than web-core's attribute latch;
//!   `autoStartRefresh` is instant where web-core's is smooth; `autoStartRefresh` without a header
//!   moves nothing (`docs/tracking/deviations.md`).
//! - The events follow native where it and web-core differ (recorded conflicts):
//!   - `offsetPercent` is native's `pullingPercent`, not web-core's `1 - scrollTop / height`
//!     (`XRefreshViewEventsEmitter.ts`, `#handleScroll`), whose `scrollTop` counts from the top
//!     placeholder, and which its `footeroffset` reuses unchanged.
//!   - `startrefresh` fires when `#container` comes to rest on the header, and at once on
//!     `autoStartRefresh`, as native enters its refreshing state; web-core fires it at the
//!     `touchend` that finds the header at least 90% visible, beside `headerreleased`, so its
//!     `autoStartRefresh` waits for the next `touchend`. `startloadmore` likewise fires at the rest
//!     on the footer, where web-core fires it at the `touchend`.
//!   - `headerreleased` and `footerreleased` need the end pulled its full height (`>= 1`), native's
//!     rule; web-core's need 90% (its `IntersectionObserver` threshold), and its `footerreleased`
//!     also fires with `startloadmore` under `enable-auto-loadmore`.
//!   - `isDragging` is the view's touch count, not native's drag state.
//!   - Native's `refreshstatechange` is not built; web-core has none.

use std::cell::{OnceCell, RefCell};

use rustc_hash::FxHashMap;
use smallvec::smallvec;
use terminus::link::event::{ElementEvent, EventPhase};
use terminus::link::scroll::{
    ScrollBehavior, ScrollIntoViewContainer, ScrollIntoViewOptions, ScrollLogicalPosition,
};
use terminus::link::{
    CustomElement, MethodCall, MethodOutcome, Node, NodeId, ShadowRootMode, ShadowSheet,
};

use super::scroll_container::{
    SCROLL_END_EVENT, SCROLL_EVENT, rect_in_scroller, rendered_scroll_box,
};
use super::touch_count::{TOUCH_CANCEL, TOUCH_END, TOUCH_START, TouchCount};
use super::{ComponentDetail, ComponentEvents, Field, LynxDocument, SLOT_CHANGE};

/// The container tag, web-core's registered spelling.
pub(super) const REFRESH_VIEW_TAG: &str = "x-refresh-view";
/// The header tag.
pub(super) const REFRESH_HEADER_TAG: &str = "x-refresh-header";
/// The footer tag.
pub(super) const REFRESH_FOOTER_TAG: &str = "x-refresh-footer";

/// The light-DOM policy, in `x-refresh-view.css`'s order; see the module
/// documentation. The host's box defaults are that file's. Each important
/// declaration sits on a line of its own, because [`super::ua_sheet`]'s
/// pinned test reads the sheet line by line.
pub(super) const UA_RULES: &str = r#"
x-refresh-view > x-refresh-header:not(:first-of-type), x-refresh-view > x-refresh-footer:not(:first-of-type),
:not(x-refresh-view) > x-refresh-header, :not(x-refresh-view) > x-refresh-footer,
x-refresh-view[enable-refresh="false"] > x-refresh-header,
x-refresh-view[enable-loadmore="false"] > x-refresh-footer { display: none; }
x-refresh-view {
  display: flex; flex-direction: column; box-sizing: border-box;
  border-width: 0; border-style: solid; position: relative; min-width: 0; min-height: 0;
}
x-refresh-view:not([enable-refresh="false"]) > x-refresh-header:first-of-type {
  flex-shrink: 0; scroll-snap-align: none;
  animation: x-refresh-header-magnet auto linear both; animation-timeline: view(block);
}
x-refresh-view:not([enable-refresh="false"]) > x-refresh-header:first-of-type { position: relative !important; }
x-refresh-view:not([enable-loadmore="false"]) > x-refresh-footer:first-of-type {
  flex-shrink: 0; margin-top: auto; scroll-snap-align: none;
  animation: x-refresh-footer-magnet auto linear both; animation-timeline: view(block);
}
x-refresh-view:not([enable-loadmore="false"]) > x-refresh-footer:first-of-type { position: relative !important; }
x-refresh-view > :not(x-refresh-header, x-refresh-footer) { overscroll-behavior-y: auto !important; }
@keyframes x-refresh-header-magnet {
  cover 0%, exit 0% { scroll-snap-align: start; }
  exit 20%, cover 100% { scroll-snap-align: none; }
}
@keyframes x-refresh-footer-magnet {
  cover 0%, entry 80% { scroll-snap-align: none; }
  entry 100%, cover 100% { scroll-snap-align: end; }
}
"#;

/// The shadow sheet, added to each instance's shadow root: see the module
/// documentation.
const SHADOW_RULES: &str = r#"
slot { display: contents; }
#container {
  display: flex; flex-direction: column; width: 100%; height: 100%; flex-shrink: 0;
  overflow-x: clip; overflow-y: scroll; overscroll-behavior: contain;
  scroll-snap-type: y mandatory; contain: content;
}
#placeholder-top, #placeholder-bot { min-height: 30%; width: 100%; flex-shrink: 0; }
#content {
  display: flex; flex-direction: column; width: 100%; height: 100%; flex-shrink: 0;
  scroll-snap-align: center; scroll-initial-target: nearest;
}
:host([enable-refresh="false"]) #placeholder-top { display: none; }
:host([enable-loadmore="false"]) #placeholder-bot { display: none; }
"#;

/// The event a `scroll` of `#container` fires while the header shows.
pub(super) const HEADER_SHOW_EVENT: &str = "headershow";
/// The event a `scroll` of `#container` fires while the header shows, the
/// same detail as [`HEADER_SHOW_EVENT`]'s, after it.
pub(super) const HEADER_OFFSET_EVENT: &str = "headeroffset";
/// The event a `scroll` of `#container` fires while the footer shows.
pub(super) const FOOTER_OFFSET_EVENT: &str = "footeroffset";
/// The event a finger lifting fires with the header pulled fully in.
pub(super) const HEADER_RELEASED_EVENT: &str = "headerreleased";
/// The event a finger lifting fires with the footer pushed fully in.
pub(super) const FOOTER_RELEASED_EVENT: &str = "footerreleased";
/// The event a refresh starting fires: `#container` coming to rest on the
/// header, or `autoStartRefresh`.
pub(super) const START_REFRESH_EVENT: &str = "startrefresh";
/// The event a load starting fires: `#container` coming to rest on the
/// footer, or, under `enable-auto-loadmore="true"`, a finger lifting with
/// the footer showing.
pub(super) const START_LOAD_MORE_EVENT: &str = "startloadmore";

/// How far, in CSS px, a rest offset may stand short of the header's (the
/// footer's) snap position and still count as resting on it. The painter
/// settles a release onto a snap position the document computed from the
/// same rounded layout the header's rectangle is read from, so a settled
/// rest normally equals the position exactly; the pixel is a margin for an
/// offset rounded anywhere between the painter's post and the document.
const LANDING_TOLERANCE: f64 = 1.0;

/// Installs the `x-refresh-view` component, which queues its events on
/// `events`. Must run before any element could carry the tag,
/// [`Document::define`](terminus::link::Document::define)'s own precondition.
pub(super) fn define(document: &mut LynxDocument, events: ComponentEvents) {
    document.define(
        REFRESH_VIEW_TAG,
        Box::new(RefreshView {
            events,
            touch_count: TouchCount::default(),
            states: RefCell::default(),
            shadow_sheet: OnceCell::new(),
        }),
    );
}

/// The `x-refresh-view` component: builds the shadow tree when the element
/// is constructed, moves the header and the footer into their named slots,
/// answers the three UI methods and fires the view's events from
/// `#container`'s `scroll` and `scrollend` and the touches on the view (the
/// module documentation's "Events").
///
/// The shadow tree is built in `constructed`, as the swiper's is: the
/// element is new and detached then, so every child arrives as a
/// `slotchange` at the default slot.
struct RefreshView {
    events: ComponentEvents,
    /// The fingers down on each view, which `isDragging` reports.
    touch_count: TouchCount,
    /// What each view's events remember, dropped when it is freed.
    states: RefCell<FxHashMap<NodeId, RefreshState>>,
    /// [`SHADOW_RULES`], parsed by the first view of the document and
    /// mounted on every view's shadow tree.
    shadow_sheet: OnceCell<ShadowSheet>,
}

/// What a refresh view's events remember between two deliveries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RefreshState {
    /// The `isManual` the next `startrefresh` reports: iOS's
    /// `manualRefresh` and web-core's `_nextRefreshIsManual`. `true` but
    /// for the length of an `autoStartRefresh` call, which clears it, sends
    /// its `startrefresh` and sets it again, as iOS's `startRefresh` does
    /// after sending (`LynxUIRefresh.m:247-262`).
    manual: bool,
    /// Whether a refresh started — `#container` came to rest on the header,
    /// or `autoStartRefresh` ran — and has not ended: a rest anywhere else
    /// and `finishRefresh` end it. No second `startrefresh` fires meanwhile.
    refreshing: bool,
    /// The same for a load: a rest on the footer, or a release under
    /// `enable-auto-loadmore`, starts it; a rest anywhere else and
    /// `finishLoadMore` end it.
    loading: bool,
}

impl Default for RefreshState {
    fn default() -> Self {
        Self {
            manual: true,
            refreshing: false,
            loading: false,
        }
    }
}

/// How far `#container`'s scrollport shows the header and the footer, from
/// the last completed layout and the document's offset. An end that
/// generates no box — none assigned, a later one, or the view's
/// `enable-refresh` / `enable-loadmore` is `"false"` — or whose height is
/// zero is `None`, and fires nothing.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Pull {
    header: Option<Shown>,
    footer: Option<Shown>,
}

/// How far one end shows: native's `pullingPercent` (`MJRefresh`), the
/// length of the end inside the scrollport over its height, measured from
/// the scrollport's start for the header and from its end for the footer.
///
/// For the header, 1 with its start on the scrollport's start, 0 with its
/// end there (scrolled out, the view at rest on `#content`), and above 1
/// while the top placeholder shows above it; the footer's the same from the
/// scrollport's end.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Shown {
    /// The `offsetPercent` reported: the CSS px lengths divided in `f64`.
    percent: f64,
    /// The end's height in CSS px, never zero.
    height: f64,
}

impl Shown {
    /// Whether any of the end shows.
    fn showing(self) -> bool {
        self.percent > 0.0
    }

    /// Whether all of the end shows: pulled at least its height.
    fn fully(self) -> bool {
        self.percent >= 1.0
    }

    /// Whether all of the end shows to within [`LANDING_TOLERANCE`]: a
    /// `scrollend` here rests on the end's snap position.
    fn rests_on(self) -> bool {
        self.percent >= 1.0 - LANDING_TOLERANCE / self.height
    }
}

impl Pull {
    /// The pull of `view`, or `None` when `#container` has no box (the view
    /// or an ancestor is `display: none`, or the view is detached).
    ///
    /// The header and the footer are each in a `display: contents` slot
    /// that is a child of `#container`, so `#container` is the parent of
    /// their boxes, and their layout locations are relative to its border
    /// box, as [`rect_in_scroller`] requires.
    fn read(document: &LynxDocument, view: NodeId) -> Option<Self> {
        let container = container(document, view)?;
        let scroll_box = rendered_scroll_box(document, container)?;
        let top = f64::from(scroll_box.offset.y);
        let bottom = top + f64::from(scroll_box.scrollport.height);
        let rect = |end: Option<NodeId>| {
            let end = end?;
            document.bounding_client_rect(end)?;
            rect_in_scroller(document, container, end).filter(|rect| rect.height() > 0.0)
        };
        Some(Self {
            header: rect(header(document, view)).map(|rect| {
                let height = f64::from(rect.height());
                Shown {
                    percent: (f64::from(rect.max_y()) - top) / height,
                    height,
                }
            }),
            footer: rect(footer(document, view)).map(|rect| {
                let height = f64::from(rect.height());
                Shown {
                    percent: (bottom - f64::from(rect.min_y())) / height,
                    height,
                }
            }),
        })
    }
}

/// `{isDragging, offsetPercent}`, the detail of the three offset events.
fn offset_detail(dragging: bool, percent: f64) -> ComponentDetail {
    ComponentDetail::Fields(smallvec![
        ("isDragging", Field::Bool(dragging)),
        ("offsetPercent", Field::Number(percent)),
    ])
}

impl RefreshView {
    /// `#container`'s `scroll`: `headershow` then `headeroffset` while the
    /// header shows, `footeroffset` while the footer shows, each only for a
    /// listener on the view.
    fn scrolled(&self, document: &LynxDocument, view: NodeId) {
        let show = document.has_listener(view, HEADER_SHOW_EVENT);
        let header_offset = document.has_listener(view, HEADER_OFFSET_EVENT);
        let footer_offset = document.has_listener(view, FOOTER_OFFSET_EVENT);
        if !(show || header_offset || footer_offset) {
            return;
        }
        let Some(pull) = Pull::read(document, view) else {
            return;
        };
        let dragging = self.touch_count.is_down(view);
        if let Some(Shown { percent, .. }) = pull.header.filter(|shown| shown.showing()) {
            for (name, wanted) in [
                (HEADER_SHOW_EVENT, show),
                (HEADER_OFFSET_EVENT, header_offset),
            ] {
                if wanted {
                    self.events
                        .queue_detailed(view, name, offset_detail(dragging, percent));
                }
            }
        }
        if let Some(Shown { percent, .. }) = pull.footer.filter(|shown| shown.showing())
            && footer_offset
        {
            self.events
                .queue_detailed(view, FOOTER_OFFSET_EVENT, offset_detail(dragging, percent));
        }
    }

    /// `#container`'s `scrollend`: a rest on the header starts a refresh,
    /// one on the footer a load, and a rest anywhere else ends whichever
    /// the other place started. The latches are recorded whether or not
    /// anything listens; each event is queued only for a listener.
    fn scroll_ended(&self, document: &LynxDocument, view: NodeId) {
        let Some(pull) = Pull::read(document, view) else {
            return;
        };
        let on_header = pull.header.is_some_and(Shown::rests_on);
        let on_footer = pull.footer.is_some_and(Shown::rests_on);
        let (refresh, load, manual) = {
            let mut states = self.states.borrow_mut();
            let state = states.entry(view).or_default();
            let refresh = on_header && !state.refreshing;
            let load = on_footer && !state.loading;
            state.refreshing = on_header;
            state.loading = on_footer;
            (refresh, load, state.manual)
        };
        if refresh && document.has_listener(view, START_REFRESH_EVENT) {
            self.events.queue_detailed(
                view,
                START_REFRESH_EVENT,
                ComponentDetail::Fields(smallvec![("isManual", Field::Bool(manual))]),
            );
        }
        if load && document.has_listener(view, START_LOAD_MORE_EVENT) {
            self.events.queue(view, START_LOAD_MORE_EVENT);
        }
    }

    /// The `touchend` the view's touch count took: `headerreleased` with the
    /// header pulled fully in, `footerreleased` with the footer pushed fully
    /// in, and under `enable-auto-loadmore="true"` a load with the footer
    /// showing at all, which latches as a rest on the footer does.
    fn released(&self, document: &LynxDocument, view: NodeId) {
        let Some(pull) = Pull::read(document, view) else {
            return;
        };
        if pull.header.is_some_and(Shown::fully)
            && document.has_listener(view, HEADER_RELEASED_EVENT)
        {
            self.events.queue(view, HEADER_RELEASED_EVENT);
        }
        if pull.footer.is_some_and(Shown::fully)
            && document.has_listener(view, FOOTER_RELEASED_EVENT)
        {
            self.events.queue(view, FOOTER_RELEASED_EVENT);
        }
        let auto = document.get(view).is_some_and(|element| {
            element.attribute("enable-auto-loadmore").as_deref() == Some("true")
        });
        if auto && pull.footer.is_some_and(Shown::showing) {
            let load = {
                let mut states = self.states.borrow_mut();
                let state = states.entry(view).or_default();
                !std::mem::replace(&mut state.loading, true)
            };
            if load && document.has_listener(view, START_LOAD_MORE_EVENT) {
                self.events.queue(view, START_LOAD_MORE_EVENT);
            }
        }
    }

    /// `autoStartRefresh()`: the instant scroll onto the header
    /// ([`auto_start_refresh`]), then, for a header that generates a box,
    /// `startrefresh` `{isManual: false}` at once, as iOS's `beginRefreshing`
    /// sends it (`LynxUIRefresh.m:380-386`), unless a refresh already
    /// started. The refresh latches, so the `scrollend` the scroll may bring
    /// on the header adds nothing.
    fn auto_start_refresh(&self, document: &mut LynxDocument, view: NodeId) {
        auto_start_refresh(document, view);
        if Pull::read(document, view).is_none_or(|pull| pull.header.is_none()) {
            return;
        }
        let refresh = {
            let mut states = self.states.borrow_mut();
            let state = states.entry(view).or_default();
            state.manual = false;
            let refresh = (!std::mem::replace(&mut state.refreshing, true)).then_some(state.manual);
            state.manual = true;
            refresh
        };
        if let Some(manual) = refresh
            && document.has_listener(view, START_REFRESH_EVENT)
        {
            self.events.queue_detailed(
                view,
                START_REFRESH_EVENT,
                ComponentDetail::Fields(smallvec![("isManual", Field::Bool(manual))]),
            );
        }
    }

    /// `finishRefresh()` (`loading == false`) or `finishLoadMore()`: the
    /// smooth scroll back to `#content` ([`finish`]), and the refresh (the
    /// load) ends, so the next rest on the header (the footer) starts one
    /// again.
    fn finish(&self, document: &mut LynxDocument, view: NodeId, loading: bool) {
        finish(document, view);
        if let Some(state) = self.states.borrow_mut().get_mut(&view) {
            if loading {
                state.loading = false;
            } else {
                state.refreshing = false;
            }
        }
    }
}

impl CustomElement<()> for RefreshView {
    /// Its own shadow tree's `slotchange`, which is how it learns that its
    /// assigned children changed; `#container`'s `scroll` and `scrollend`,
    /// which its events are decided on; and the touches on it, which
    /// `isDragging` counts and whose `touchend` releases a pull.
    fn observed_events(&self) -> &'static [&'static str] {
        &[
            SLOT_CHANGE,
            SCROLL_EVENT,
            SCROLL_END_EVENT,
            TOUCH_START,
            TOUCH_END,
            TOUCH_CANCEL,
        ]
    }

    fn disconnected_callback(&self, _document: &LynxDocument, element: NodeId) {
        self.touch_count.forget(element);
    }

    fn freed(&self, element: NodeId) {
        self.states.borrow_mut().remove(&element);
        self.touch_count.forget(element);
    }

    fn constructed(&self, document: &mut LynxDocument, element: NodeId) {
        let shadow = document.attach_shadow(element, ShadowRootMode::Open);
        let sheet = self
            .shadow_sheet
            .get_or_init(|| document.parse_shadow_stylesheet(SHADOW_RULES));
        document.add_shadow_sheet(shadow, sheet);
        let container = div(document, "container");
        let top = div(document, "placeholder-top");
        let header = named_slot(document, "header");
        let content = div(document, "content");
        let slot = document.create_element("slot", ());
        document.append_child(content, slot);
        let footer = named_slot(document, "footer");
        let bottom = div(document, "placeholder-bot");
        for part in [top, header, content, footer, bottom] {
            document.append_child(container, part);
        }
        document.append_child(shadow, container);
    }

    /// `slotchange` at the default slot — the one slot with no `name` —
    /// delivered at the shadow root once per pass: the capture delivery is
    /// skipped, the bubble one acts. The named slots' own are ignored.
    ///
    /// `scroll` and `scrollend` whose target is `#container`, delivered at
    /// the shadow root too, in the capture pass alone, since neither
    /// bubbles; the same names aimed anywhere else — a scroller inside the
    /// content, whose capture pass crosses the view — are not its own.
    /// Every touch event goes to its count, and the `touchend` the count
    /// takes releases the pull.
    fn handle_event(
        &self,
        document: &mut LynxDocument,
        element: NodeId,
        event: &mut ElementEvent<'_>,
    ) {
        match event.type_name() {
            SLOT_CHANGE => {
                if event.phase() == EventPhase::Bubbling {
                    slot_ends(document, event.target());
                }
            }
            name @ (SCROLL_EVENT | SCROLL_END_EVENT) => {
                if container(document, element) != Some(event.target()) {
                    return;
                }
                if name == SCROLL_EVENT {
                    self.scrolled(document, element);
                } else {
                    self.scroll_ended(document, element);
                }
            }
            name => {
                if self.touch_count.observe(element, event) && name == TOUCH_END {
                    self.released(document, element);
                }
            }
        }
    }

    /// `finishRefresh`, `finishLoadMore` and `autoStartRefresh`; see the
    /// module documentation. None reads `params`, and each answers
    /// [`MethodOutcome::Done`], including when it has nothing to scroll.
    fn invoke(
        &self,
        document: &mut LynxDocument,
        element: NodeId,
        call: MethodCall<'_>,
    ) -> MethodOutcome {
        match call.name {
            "finishRefresh" => self.finish(document, element, false),
            "finishLoadMore" => self.finish(document, element, true),
            "autoStartRefresh" => self.auto_start_refresh(document, element),
            _ => return MethodOutcome::NotFound,
        }
        MethodOutcome::Done
    }
}

/// The default slot's `slotchange` (`slot` being the event's target): every
/// `x-refresh-header` assigned there gets `slot="header"` and every
/// `x-refresh-footer` `slot="footer"`. A named slot's own changes nothing.
fn slot_ends(document: &mut LynxDocument, slot: NodeId) {
    if document
        .get(slot)
        .is_none_or(|node| node.attribute("name").is_some())
    {
        return;
    }
    let moves: Vec<(NodeId, &str)> = document
        .assigned_nodes(slot)
        .iter()
        .filter_map(|&node| match document.get(node).and_then(Node::tag_name)? {
            REFRESH_HEADER_TAG => Some((node, "header")),
            REFRESH_FOOTER_TAG => Some((node, "footer")),
            _ => None,
        })
        .collect();
    for (node, name) in moves {
        document.set_attribute(node, "slot", name);
    }
}

/// `finishRefresh()` and `finishLoadMore()` on `view`: a smooth scroll of
/// `#container` that puts `#content` back at its snap position, the
/// scrollport's center. Nothing is recorded when `#container` already rests
/// there, or when the view has no box.
fn finish(document: &mut LynxDocument, view: NodeId) {
    if let Some(content) = content(document, view) {
        scroll_into_view(
            document,
            content,
            ScrollLogicalPosition::Center,
            ScrollBehavior::Smooth,
        );
    }
}

/// `autoStartRefresh()` on `view`: an instant scroll of `#container` that
/// puts the header's start on the scrollport's start, the header's own snap
/// position once it is shown. Nothing is recorded when the view has no
/// header or the header generates no box (`enable-refresh="false"`).
///
/// Instant rather than web-core's smooth: from rest the header is no snap
/// position, and the painter settles a smooth request's target onto the
/// frame's snap positions when it receives it, which would turn it back to
/// `#content`; see the module documentation. The `startrefresh` it brings
/// is the component's ([`RefreshView::auto_start_refresh`]).
fn auto_start_refresh(document: &mut LynxDocument, view: NodeId) {
    if let Some(header) = header(document, view) {
        scroll_into_view(
            document,
            header,
            ScrollLogicalPosition::Start,
            ScrollBehavior::Instant,
        );
    }
}

/// Scrolls `target` into view with `behavior`, at `block` on y and
/// `nearest` on x, in its nearest scroll container: `#container`, the
/// flat-tree scroll container of the header, the footer and `#content`. Its
/// `overflow` is a shadow-sheet declaration no author rule reaches, so no
/// ancestor of the view can be scrolled instead.
fn scroll_into_view(
    document: &mut LynxDocument,
    target: NodeId,
    block: ScrollLogicalPosition,
    behavior: ScrollBehavior,
) {
    document.scroll_into_view(
        target,
        ScrollIntoViewOptions {
            behavior,
            block,
            inline: ScrollLogicalPosition::Nearest,
            container: ScrollIntoViewContainer::Nearest,
        },
    );
}

/// A `div` with `id`.
fn div(document: &mut LynxDocument, id: &str) -> NodeId {
    let div = document.create_element("div", ());
    document.set_id_attribute(div, Some(id));
    div
}

/// A `<slot>` named `name`.
fn named_slot(document: &mut LynxDocument, name: &str) -> NodeId {
    let slot = document.create_element("slot", ());
    document.set_attribute(slot, "name", name);
    slot
}

/// The refresh view's scroll container, `#container`, the one element of
/// its shadow tree.
pub(crate) fn container(document: &LynxDocument, view: NodeId) -> Option<NodeId> {
    let root = document.shadow_root(view)?;
    document
        .get(root)?
        .children()
        .find(|child| child.is_element())
        .map(Node::id)
}

/// The child of `#container` that is `#content`, the box holding the
/// default slot.
fn content(document: &LynxDocument, view: NodeId) -> Option<NodeId> {
    let container = container(document, view)?;
    document
        .get(container)?
        .children()
        .find(|child| child.attribute("id").as_deref() == Some("content"))
        .map(Node::id)
}

/// The header: the first element assigned to `#container`'s `header`
/// slot, which is the view's first `x-refresh-header` child once the
/// component has set its `slot`.
fn header(document: &LynxDocument, view: NodeId) -> Option<NodeId> {
    slotted(document, view, "header")
}

/// The footer: the first element assigned to `#container`'s `footer`
/// slot, which is the view's first `x-refresh-footer` child once the
/// component has set its `slot`.
fn footer(document: &LynxDocument, view: NodeId) -> Option<NodeId> {
    slotted(document, view, "footer")
}

/// The first element assigned to the slot of `#container` named `name`.
fn slotted(document: &LynxDocument, view: NodeId, name: &str) -> Option<NodeId> {
    let container = container(document, view)?;
    let slot = document
        .get(container)?
        .children()
        .find(|child| {
            child.tag_name() == Some("slot") && child.attribute("name").as_deref() == Some(name)
        })?
        .id();
    document
        .assigned_nodes(slot)
        .iter()
        .copied()
        .find(|&node| document.get(node).is_some_and(Node::is_element))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::float_cmp)] // Explicit pixel sizes lay out exactly.

    use terminus::link::scroll::ScrollBehavior;
    use terminus::link::stylo::properties::PropertyId;
    use terminus::link::stylo::values::computed::Display;
    use terminus::link::{MethodCall, MethodOutcome, NodeId, StylesheetOrigin, Vector2D};

    use super::super::test_support::{
        child, document, element_under, style_of, with_component_events,
    };
    use super::super::{ComponentEvents, LynxDocument, PageConfig};
    use super::{REFRESH_FOOTER_TAG, REFRESH_HEADER_TAG, REFRESH_VIEW_TAG};

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

    fn display(document: &LynxDocument, element: NodeId) -> Display {
        style_of(document, element).slow_clone_display()
    }

    /// A 393px x 800px refresh view with `attributes`, a 50px header, one
    /// content `view` of the view's full height, and a 50px footer.
    struct Built {
        document: LynxDocument,
        /// What the document's components queued for script.
        events: ComponentEvents,
        view: NodeId,
        header: NodeId,
        content: NodeId,
        footer: NodeId,
    }

    impl Built {
        fn new(attributes: &[(&str, &str)], content_tag: &str) -> Self {
            Self::with_config(PageConfig::default(), attributes, content_tag)
        }

        fn with_config(config: PageConfig, attributes: &[(&str, &str)], content_tag: &str) -> Self {
            let (mut document, events) = with_component_events(config);
            let view = child(
                &mut document,
                REFRESH_VIEW_TAG,
                "width: 393px; height: 800px",
            );
            for (name, value) in attributes {
                document.set_attribute(view, name, value);
            }
            let header = element_under(&mut document, view, REFRESH_HEADER_TAG, "height: 50px");
            let content = element_under(&mut document, view, content_tag, "height: 100%");
            let footer = element_under(&mut document, view, REFRESH_FOOTER_TAG, "height: 50px");
            Self {
                document,
                events,
                view,
                header,
                content,
                footer,
            }
        }

        /// `#container`, the shadow scroll container.
        fn container(&self) -> NodeId {
            super::container(&self.document, self.view).expect("every refresh view has one")
        }

        /// `#container`'s shadow children: the two placeholders, the two
        /// named slots and `#content`, in tree order.
        fn parts(&self) -> Vec<NodeId> {
            self.document
                .get(self.container())
                .expect("live")
                .child_ids()
                .to_vec()
        }

        /// `#container`'s snap positions on y, as single offsets.
        fn snap_points(&self) -> Vec<f32> {
            let positions = self
                .document
                .snap_positions(self.container())
                .expect("a snapping container");
            positions
                .y()
                .expect("snaps on y")
                .points
                .iter()
                .map(|point| {
                    assert_eq!(point.min, point.max, "no snap area outgrows the port");
                    point.min
                })
                .collect()
        }

        /// Scrolls `#container` to `y` as an adopted painter post does,
        /// and re-samples what follows the scroll.
        fn scroll_to(&mut self, y: f32) {
            let container = self.container();
            self.document.scroll_to(container, Vector2D::new(0.0, y));
            self.document.advance_scroll_timelines(&[container]);
            self.document.commit();
        }

        /// Commits, and acknowledges the request the first commit's initial
        /// scroll target left pending, as the painter's first post for
        /// `#container` does: the view at rest on what it opened on.
        fn at_rest(&mut self) {
            self.document.commit();
            let container = self.container();
            if let Some(serial) = self.document.pending_scroll_request(container) {
                self.document.acknowledge_scroll_request(container, serial);
            }
        }

        /// Runs the UI method `name` on the view.
        fn invoke(&mut self, name: &str, params: &str) -> MethodOutcome {
            self.document
                .invoke_element_method(self.view, MethodCall { name, params })
        }

        /// `#container`'s offset in the document.
        fn offset(&self) -> Vector2D<f32> {
            self.document.scroll_offset(self.container())
        }

        /// The scroll request the next commit carries to the painter for
        /// `#container`. With none pending, a commit of an unchanged
        /// document hands back the last frame, which still names the
        /// request it carried; so the pending state is read first.
        fn pending(&mut self) -> Option<(Vector2D<f32>, ScrollBehavior)> {
            let container = self.container();
            self.document.pending_scroll_request(container)?;
            let frame = self.document.commit();
            let index = frame
                .slot_of(container)
                .expect("`#container` has a scroll slot");
            frame.scroll_slots()[index as usize]
                .request
                .map(|request| (request.target, request.behavior))
        }
    }

    // --- the shadow tree ----------------------------------------------------

    /// `#container` holds the top placeholder, the header slot, `#content`
    /// with the default slot, the footer slot and the bottom placeholder.
    /// The header and the footer get their `slot` attributes from the
    /// component and land in the named slots; the content child stays in
    /// the default one. The light DOM is untouched otherwise.
    #[test]
    fn the_header_and_footer_are_slotted_around_the_content() {
        let mut built = Built::new(&[], "view");
        built.document.layout();
        let document = &built.document;
        let parts = built.parts();
        let ids: Vec<_> = parts
            .iter()
            .map(|part| {
                let node = document.get(*part).expect("live");
                (
                    node.tag_name().map(str::to_owned),
                    node.attribute("id")
                        .or(node.attribute("name"))
                        .map(std::borrow::Cow::into_owned),
                )
            })
            .collect();
        let named = |tag: &str, name: &str| (Some(tag.to_owned()), Some(name.to_owned()));
        assert_eq!(
            ids,
            [
                named("div", "placeholder-top"),
                named("slot", "header"),
                named("div", "content"),
                named("slot", "footer"),
                named("div", "placeholder-bot"),
            ]
        );
        let default_slot = document.get(parts[2]).expect("live").child_ids()[0];
        assert_eq!(document.assigned_nodes(parts[1]), [built.header]);
        assert_eq!(document.assigned_nodes(default_slot), [built.content]);
        assert_eq!(document.assigned_nodes(parts[3]), [built.footer]);
        let slot_of = |node: NodeId| document.get(node).and_then(|node| node.attribute("slot"));
        assert_eq!(slot_of(built.header).as_deref(), Some("header"));
        assert_eq!(slot_of(built.footer).as_deref(), Some("footer"));
        assert_eq!(slot_of(built.content), None);
        for slot in [parts[1], default_slot, parts[3]] {
            assert_eq!(display(document, slot), Display::Contents);
        }
        assert_eq!(
            document.get(built.view).expect("live").child_ids(),
            [built.header, built.content, built.footer]
        );
        assert_eq!(
            document.scroll_box(built.view),
            None,
            "the host does not scroll"
        );
    }

    /// A second header or footer is slotted too, but only the first of each
    /// generates a box (`:first-of-type`), and a header outside a refresh
    /// view generates none.
    #[test]
    fn only_the_first_header_and_footer_generate_a_box() {
        let mut built = Built::new(&[], "view");
        let second_header = element_under(&mut built.document, built.view, REFRESH_HEADER_TAG, "");
        let second_footer = element_under(&mut built.document, built.view, REFRESH_FOOTER_TAG, "");
        let stray = child(&mut built.document, REFRESH_HEADER_TAG, "height: 50px");
        built.document.layout();
        let document = &built.document;
        let slot_of = |node: NodeId| document.get(node).and_then(|node| node.attribute("slot"));
        assert_eq!(slot_of(second_header).as_deref(), Some("header"));
        assert_eq!(slot_of(second_footer).as_deref(), Some("footer"));
        assert_eq!(slot_of(stray), None);
        for (element, expected) in [
            (built.header, Display::Linear),
            (built.footer, Display::Linear),
            (second_header, Display::None),
            (second_footer, Display::None),
            (stray, Display::None),
        ] {
            assert_eq!(display(document, element), expected);
        }
    }

    /// A shown header and footer take the engine's display policy:
    /// `linear` under `defaultDisplayLinear`, the engine default (`flex`)
    /// without it.
    #[test]
    fn the_header_and_footer_follow_the_display_switch() {
        for (linear, expected) in [(true, Display::Linear), (false, Display::Flex)] {
            let config = PageConfig {
                default_display_linear: linear,
                ..PageConfig::default()
            };
            let mut built = Built::with_config(config, &[], "view");
            built.document.layout();
            for end in [built.header, built.footer] {
                assert_eq!(display(&built.document, end), expected, "linear={linear}");
            }
        }
    }

    // --- the attributes -----------------------------------------------------

    /// `"false"` hides the header (footer) and the placeholder above
    /// (below) it; `"true"`, any other value and no attribute show them.
    #[test]
    fn enable_refresh_and_enable_loadmore_false_hide_their_ends() {
        for (attribute, shown) in [
            (None, true),
            (Some("true"), true),
            (Some(""), true),
            (Some("false"), false),
        ] {
            for (name, end, placeholder) in [("enable-refresh", 0, 0), ("enable-loadmore", 2, 4)] {
                let attributes: Vec<_> = attribute.map(|value| (name, value)).into_iter().collect();
                let mut built = Built::new(&attributes, "view");
                built.document.layout();
                let ends = [built.header, built.content, built.footer];
                let expected = if shown {
                    Display::Linear
                } else {
                    Display::None
                };
                assert_eq!(
                    display(&built.document, ends[end]),
                    expected,
                    "{name}={attribute:?}"
                );
                let placeholder = built.parts()[placeholder];
                assert_eq!(
                    display(&built.document, placeholder) != Display::None,
                    shown,
                    "{name}={attribute:?}"
                );
            }
        }
    }

    // --- the geometry -------------------------------------------------------

    /// The scroll range is the two 240px placeholders (30% of 800), the
    /// header, `#content` (800) and the footer: 1380. The first commit
    /// opens on `#content` at 290. Without the top end, it opens at 0.
    #[test]
    fn the_view_opens_on_its_content_with_the_ends_out_of_view() {
        for (attributes, height, offset) in [
            (vec![], 1380.0, 290.0),
            (vec![("enable-refresh", "false")], 1090.0, 0.0),
            (vec![("enable-loadmore", "false")], 1090.0, 290.0),
        ] {
            let mut built = Built::new(&attributes, "view");
            built.document.commit();
            let container = built.container();
            let scroll_box = built
                .document
                .scroll_box(container)
                .expect("`#container` scrolls");
            assert_eq!(scroll_box.scrollport.height, 800.0, "{attributes:?}");
            assert_eq!(scroll_box.scroll_size.height, height, "{attributes:?}");
            assert_eq!(
                built.document.scroll_offset(container),
                Vector2D::new(0.0, offset),
                "{attributes:?}"
            );
            let content = built
                .document
                .rounded_layout(built.content)
                .expect("laid out");
            assert_eq!(
                (content.size.width, content.size.height),
                (393.0, 800.0),
                "{attributes:?}"
            );
        }
    }

    /// An author `position: absolute` on the header, as web-core's demo card
    /// writes it, loses to the UA's `!important`: the header stays in the
    /// column and the range keeps its 50px.
    #[test]
    fn an_author_absolute_header_stays_in_the_column() {
        let mut built = Built::new(&[], "view");
        built.document.add_stylesheet(
            "x-refresh-header { position: absolute !important; }",
            StylesheetOrigin::Author,
        );
        built
            .document
            .set_inline_style(built.header, "height: 50px; position: absolute");
        built.document.commit();
        assert_eq!(value(&built.document, built.header, "position"), "relative");
        assert_eq!(value(&built.document, built.footer, "position"), "relative");
        let container = built.container();
        let scroll_box = built.document.scroll_box(container).expect("scrolls");
        assert_eq!(scroll_box.scroll_size.height, 1380.0);
        assert_eq!(
            built.document.scroll_offset(container),
            Vector2D::new(0.0, 290.0)
        );
    }

    // --- the snap alignment -------------------------------------------------

    /// At rest only `#content` snaps (290). A header at least 90% visible
    /// snaps at its start (240) and a footer at least 90% visible at its
    /// end (340); one less visible snaps nowhere.
    #[test]
    fn a_header_or_footer_pulled_nine_tenths_in_becomes_a_snap_position() {
        let mut built = Built::new(&[], "view");
        built.document.commit();
        assert_eq!(built.snap_points(), [290.0]);
        assert_eq!(
            value(&built.document, built.header, "scroll-snap-align"),
            "none"
        );
        assert_eq!(
            value(&built.document, built.footer, "scroll-snap-align"),
            "none"
        );
        assert_eq!(
            value(&built.document, built.content, "scroll-snap-align"),
            "none"
        );

        // Fully in view, then 92% (header top 4px above the scrollport).
        for y in [190.0, 0.0, 244.0] {
            built.scroll_to(y);
            assert_eq!(
                value(&built.document, built.header, "scroll-snap-align"),
                "start",
                "at {y}"
            );
            assert_eq!(built.snap_points(), [240.0, 290.0], "at {y}");
        }
        // 80% in view: `exit 20%`.
        built.scroll_to(250.0);
        assert_eq!(
            value(&built.document, built.header, "scroll-snap-align"),
            "none"
        );
        assert_eq!(built.snap_points(), [290.0]);

        for y in [490.0, 580.0, 336.0] {
            built.scroll_to(y);
            assert_eq!(
                value(&built.document, built.footer, "scroll-snap-align"),
                "end",
                "at {y}"
            );
            assert_eq!(built.snap_points(), [290.0, 340.0], "at {y}");
        }
        built.scroll_to(330.0);
        assert_eq!(
            value(&built.document, built.footer, "scroll-snap-align"),
            "none"
        );
        assert_eq!(built.snap_points(), [290.0]);
    }

    /// The alignment animations export nothing to the painter and need no
    /// main-thread tick: they re-sample when a scroll is adopted.
    #[test]
    fn the_snap_alignment_needs_no_ticks() {
        let mut built = Built::new(&[], "view");
        let frame = built.document.commit();
        assert!(frame.animation_slots().is_empty());
        assert!(!frame.needs_main_ticks() && !frame.has_live_curves());
    }

    // --- the scroll chain ---------------------------------------------------

    /// `#container` contains its chain; the content child, a scroller or
    /// not, chains on y whatever an author declares on it, and keeps its
    /// own x.
    #[test]
    fn the_content_child_chains_on_y_into_the_container() {
        let mut built = Built::new(&[], "scroll-view");
        built.document.set_attribute(built.content, "bounces", "");
        built.document.add_stylesheet(
            "scroll-view { overscroll-behavior: contain-bounce !important; }",
            StylesheetOrigin::Author,
        );
        built.document.layout();
        let container = built.container();
        for (property, expected) in [
            ("overscroll-behavior-x", "contain"),
            ("overscroll-behavior-y", "contain"),
            ("scroll-snap-type", "y mandatory"),
            ("contain", "content"),
        ] {
            assert_eq!(
                value(&built.document, container, property),
                expected,
                "{property}"
            );
        }
        assert_eq!(
            value(&built.document, built.content, "overscroll-behavior-y"),
            "auto"
        );
        assert_eq!(
            value(&built.document, built.content, "overscroll-behavior-x"),
            "contain-bounce"
        );
        for chrome in [built.header, built.footer] {
            assert_eq!(
                value(&built.document, chrome, "overscroll-behavior-y"),
                "auto"
            );
        }
    }

    /// The scroll chain from the slotted content child runs over the flat
    /// tree into `#container`: a pull past the inner scroller's top moves
    /// `#container`, and a push past its end moves it the other way.
    #[test]
    fn a_pull_on_the_content_scrolls_the_container() {
        let mut built = Built::new(&[], "scroll-view");
        let tall = element_under(
            &mut built.document,
            built.content,
            "view",
            "flex-shrink: 0; height: 1000px",
        );
        built.document.commit();
        let container = built.container();
        let moved = built
            .document
            .scroll_chain(tall, Vector2D::new(0.0, -100.0));
        assert_eq!(moved, Some((container, Vector2D::new(0.0, -100.0))));
        assert_eq!(
            built.document.scroll_offset(container),
            Vector2D::new(0.0, 190.0)
        );
        assert_eq!(
            built.document.scroll_offset(built.content),
            Vector2D::zero()
        );

        let moved = built.document.scroll_chain(tall, Vector2D::new(0.0, 400.0));
        assert_eq!(
            moved,
            Some((built.content, Vector2D::new(0.0, 400.0))),
            "the inner scroller goes first: 200 of its own, 200 into the container"
        );
        assert_eq!(
            built.document.scroll_offset(built.content),
            Vector2D::new(0.0, 200.0)
        );
        assert_eq!(
            built.document.scroll_offset(container),
            Vector2D::new(0.0, 390.0)
        );
    }

    // --- the UI methods -----------------------------------------------------

    /// `autoStartRefresh` moves `#container` at once to the header's start
    /// snap position, 240, and records the instant request the painter
    /// follows. The commit that carries it publishes the header's `start`
    /// alignment, so 240 is a snap position the painter's at-rest rule
    /// leaves alone.
    #[test]
    fn auto_start_refresh_moves_the_header_to_its_snap_position_at_once() {
        let mut built = Built::new(&[], "view");
        built.at_rest();
        assert_eq!(built.invoke("autoStartRefresh", "{}"), MethodOutcome::Done);
        assert_eq!(built.offset(), Vector2D::new(0.0, 240.0), "at once");
        assert_eq!(
            built.pending(),
            Some((Vector2D::new(0.0, 240.0), ScrollBehavior::Instant))
        );
        assert_eq!(
            value(&built.document, built.header, "scroll-snap-align"),
            "start"
        );
        assert_eq!(built.snap_points(), [240.0, 290.0]);
    }

    /// With the header (240) or the footer (340) shown, either finishing
    /// method records a smooth request back to `#content` at 290.
    #[test]
    fn finish_refresh_and_finish_load_more_scroll_the_content_back() {
        for (y, shown) in [(240.0, "header"), (340.0, "footer")] {
            for name in ["finishRefresh", "finishLoadMore"] {
                let mut built = Built::new(&[], "view");
                built.at_rest();
                built.scroll_to(y);
                assert_eq!(built.invoke(name, "{}"), MethodOutcome::Done, "{name}");
                assert_eq!(
                    built.offset(),
                    Vector2D::new(0.0, y),
                    "{name} from the {shown}"
                );
                assert_eq!(
                    built.pending(),
                    Some((Vector2D::new(0.0, 290.0), ScrollBehavior::Smooth)),
                    "{name} from the {shown}"
                );
            }
        }
    }

    /// A view resting on `#content` records nothing for either finishing
    /// method.
    #[test]
    fn finishing_at_the_content_records_nothing() {
        let mut built = Built::new(&[], "view");
        built.at_rest();
        for name in ["finishRefresh", "finishLoadMore"] {
            assert_eq!(built.invoke(name, "{}"), MethodOutcome::Done, "{name}");
            assert_eq!(built.offset(), Vector2D::new(0.0, 290.0), "{name}");
            assert_eq!(built.pending(), None, "{name}");
        }
    }

    /// `enable-refresh="false"` gives the header no box: `autoStartRefresh`
    /// answers `Done` and records nothing.
    #[test]
    fn auto_start_refresh_without_a_header_box_records_nothing() {
        let mut built = Built::new(&[("enable-refresh", "false")], "view");
        built.at_rest();
        assert_eq!(built.invoke("autoStartRefresh", "{}"), MethodOutcome::Done);
        assert_eq!(built.offset(), Vector2D::zero());
        assert_eq!(built.pending(), None);
    }

    /// A view with no header child: `autoStartRefresh` answers `Done` and
    /// records nothing.
    #[test]
    fn auto_start_refresh_without_a_header_records_nothing() {
        let mut document = document();
        let view = child(
            &mut document,
            REFRESH_VIEW_TAG,
            "width: 393px; height: 800px",
        );
        element_under(&mut document, view, "view", "height: 100%");
        element_under(&mut document, view, REFRESH_FOOTER_TAG, "height: 50px");
        document.commit();
        let container = super::container(&document, view).expect("every refresh view has one");
        if let Some(serial) = document.pending_scroll_request(container) {
            document.acknowledge_scroll_request(container, serial);
        }
        let at_rest = document.scroll_offset(container);
        assert_eq!(
            at_rest,
            Vector2D::new(0.0, 240.0),
            "below the top placeholder"
        );
        assert_eq!(
            document.invoke_element_method(
                view,
                MethodCall {
                    name: "autoStartRefresh",
                    params: "{}",
                }
            ),
            MethodOutcome::Done
        );
        assert_eq!(document.scroll_offset(container), at_rest);
        assert_eq!(document.pending_scroll_request(container), None);
    }

    /// `invoke` answers the three names with `Done` whatever `params` is,
    /// on a laid-out view and on a `display: none` one, and `NotFound` for
    /// every other name, which leaves it to the base set.
    #[test]
    fn invoke_answers_the_three_methods_and_nothing_else() {
        for style in ["", "display: none"] {
            let mut built = Built::new(&[], "view");
            if !style.is_empty() {
                built.document.set_inline_style(built.view, style);
            }
            built.at_rest();
            for name in ["finishRefresh", "finishLoadMore", "autoStartRefresh"] {
                for params in ["", "null", "{}"] {
                    assert_eq!(
                        built.invoke(name, params),
                        MethodOutcome::Done,
                        "{name}({params:?}) on {style:?}"
                    );
                }
            }
            for name in ["selectTab", "scrollTo", "startRefresh", "finishrefresh"] {
                assert_eq!(
                    built.invoke(name, "{}"),
                    MethodOutcome::NotFound,
                    "{name} on {style:?}"
                );
            }
            let container = built.container();
            if style.is_empty() {
                assert!(
                    built.document.pending_scroll_request(container).is_some(),
                    "the last call, `autoStartRefresh`, is pending"
                );
            } else {
                assert_eq!(built.document.scroll_box(container), None, "no box");
                assert_eq!(built.document.pending_scroll_request(container), None);
            }
        }
    }

    // --- the events ---------------------------------------------------------

    use terminus::link::event::EventDetail;

    use super::super::scroll_container::tests::{listen, scrolled};
    use super::super::test_support::touched;
    use super::super::touch_count::{TOUCH_END, TOUCH_START};
    use super::super::{ComponentDetail, ComponentEvent, Field};
    use super::{
        FOOTER_OFFSET_EVENT, FOOTER_RELEASED_EVENT, HEADER_OFFSET_EVENT, HEADER_RELEASED_EVENT,
        HEADER_SHOW_EVENT, Pull, START_LOAD_MORE_EVENT, START_REFRESH_EVENT, Shown,
    };

    /// Every event the view fires.
    const EVERY_EVENT: [&str; 7] = [
        HEADER_SHOW_EVENT,
        HEADER_OFFSET_EVENT,
        FOOTER_OFFSET_EVENT,
        HEADER_RELEASED_EVENT,
        FOOTER_RELEASED_EVENT,
        START_REFRESH_EVENT,
        START_LOAD_MORE_EVENT,
    ];

    /// One queued event as `(name, detail pairs)`; a `{}` detail has none.
    type Heard = (&'static str, Vec<(&'static str, Field)>);

    impl Built {
        /// The view at rest on `#content` (290), with listeners on the view
        /// for `names`.
        fn listening(attributes: &[(&str, &str)], names: &[&str]) -> Self {
            let mut built = Self::new(attributes, "view");
            built.at_rest();
            for name in names {
                listen(&mut built.document, built.view, name);
            }
            built
        }

        /// Moves `#container` to `y` and delivers its `scroll`, as the
        /// runtime delivers the kernel's: at `#container`, which ends at the
        /// shadow root.
        fn pulled(&mut self, y: f32) {
            let container = self.container();
            scrolled(&mut self.document, container, Vector2D::new(0.0, y));
        }

        /// Moves `#container` to `y` and delivers its `scrollend` there: a
        /// rest at `y`.
        fn rests(&mut self, y: f32) {
            let container = self.container();
            self.document.scroll_to(container, Vector2D::new(0.0, y));
            self.document.dispatch_element_event(
                container,
                "scrollend",
                &EventDetail::None,
                false,
                false,
                0.0,
            );
        }

        /// A finger going down (`touchstart`) or lifting (`touchend`) on
        /// the content child, as the runtime dispatches a routed touch.
        fn touch(&mut self, name: &'static str) {
            touched(&mut self.document, self.content, name);
        }

        /// What the view queued since the last call, in order.
        fn heard(&self) -> Vec<Heard> {
            self.events
                .take()
                .into_iter()
                .map(|event| match event {
                    ComponentEvent::Detailed {
                        node,
                        name,
                        detail: ComponentDetail::Fields(fields),
                    } => {
                        assert_eq!(node, self.view, "{name} is queued at the view");
                        (name, fields.into_vec())
                    }
                    ComponentEvent::Plain { node, name } => {
                        assert_eq!(node, self.view, "{name} is queued at the view");
                        (name, Vec::new())
                    }
                    other => panic!("the view queues fields and `{{}}` events: {other:?}"),
                })
                .collect()
        }
    }

    fn offset(name: &'static str, dragging: bool, percent: f64) -> Heard {
        (
            name,
            vec![
                ("isDragging", Field::Bool(dragging)),
                ("offsetPercent", Field::Number(percent)),
            ],
        )
    }

    fn start_refresh(manual: bool) -> Heard {
        (START_REFRESH_EVENT, vec![("isManual", Field::Bool(manual))])
    }

    fn plain(name: &'static str) -> Heard {
        (name, Vec::new())
    }

    /// The header and the footer are boxes of `#container` through their
    /// `display: contents` slots, so their rectangles in its scroll
    /// coordinates are their layout locations: the header after the 240px
    /// top placeholder, the footer after the header and the 800px
    /// `#content`. The view at rest shows neither.
    #[test]
    fn the_header_and_footer_are_measured_in_the_containers_coordinates() {
        let mut built = Built::new(&[], "view");
        built.at_rest();
        let container = built.container();
        let rect = |end| {
            let rect =
                super::super::scroll_container::rect_in_scroller(&built.document, container, end)
                    .expect("laid out");
            (rect.min_x(), rect.min_y(), rect.width(), rect.height())
        };
        assert_eq!(rect(built.header), (0.0, 240.0, 393.0, 50.0));
        assert_eq!(rect(built.footer), (0.0, 1090.0, 393.0, 50.0));
        let shown = |percent| {
            Some(Shown {
                percent,
                height: 50.0,
            })
        };
        assert_eq!(
            Pull::read(&built.document, built.view),
            Some(Pull {
                header: shown(0.0),
                footer: shown(0.0),
            })
        );
    }

    /// With no listener on the view, nothing is queued: not on a pull, a
    /// release, a rest on either end, nor `autoStartRefresh`.
    #[test]
    fn nothing_is_queued_without_a_listener() {
        let mut built = Built::listening(&[], &[]);
        built.touch(TOUCH_START);
        built.pulled(200.0);
        built.touch(TOUCH_END);
        built.rests(240.0);
        built.rests(290.0);
        built.touch(TOUCH_START);
        built.pulled(400.0);
        built.touch(TOUCH_END);
        built.rests(340.0);
        built.rests(290.0);
        assert_eq!(built.invoke("autoStartRefresh", "{}"), MethodOutcome::Done);
        assert_eq!(built.heard(), []);
    }

    /// `headershow` then `headeroffset` on each `scroll` that leaves the
    /// header showing, with native's `pullingPercent`: 0.5 halfway in, 1
    /// with its start on the scrollport's start, 5.8 with the whole 240px
    /// top placeholder showing above it. A `scroll` leaving it out, at rest
    /// on `#content` or past it, fires nothing.
    #[test]
    fn header_show_and_offset_report_how_far_the_header_is_pulled() {
        let mut built = Built::listening(&[], &[HEADER_SHOW_EVENT, HEADER_OFFSET_EVENT]);
        for y in [290.0, 300.0] {
            built.pulled(y);
            assert_eq!(built.heard(), [], "scrolled out at {y}");
        }
        for (y, percent) in [(265.0, 0.5), (240.0, 1.0), (0.0, 5.8)] {
            built.pulled(y);
            assert_eq!(
                built.heard(),
                [
                    offset(HEADER_SHOW_EVENT, false, percent),
                    offset(HEADER_OFFSET_EVENT, false, percent),
                ],
                "at {y}"
            );
        }
    }

    /// `isDragging` is whether a finger is down on the view, as each
    /// `scroll` is delivered; each event is queued only for its own
    /// listener.
    #[test]
    fn is_dragging_is_whether_a_finger_is_down() {
        let mut built = Built::listening(&[], &[HEADER_OFFSET_EVENT]);
        built.touch(TOUCH_START);
        built.pulled(280.0);
        assert_eq!(
            built.heard(),
            [offset(HEADER_OFFSET_EVENT, true, 0.2)],
            "no `headershow` listener"
        );
        built.touch(TOUCH_END);
        built.pulled(270.0);
        assert_eq!(built.heard(), [offset(HEADER_OFFSET_EVENT, false, 0.4)]);
    }

    /// A `scroll` of a scroller inside the view crosses it in the capture
    /// pass, and is not `#container`'s: nothing.
    #[test]
    fn an_inner_scrollers_scroll_is_not_the_containers() {
        let mut built = Built::new(&[], "scroll-view");
        element_under(
            &mut built.document,
            built.content,
            "view",
            "flex-shrink: 0; height: 1000px",
        );
        built.at_rest();
        listen(&mut built.document, built.view, HEADER_OFFSET_EVENT);
        let container = built.container();
        built
            .document
            .scroll_to(container, Vector2D::new(0.0, 265.0));
        scrolled(&mut built.document, built.content, Vector2D::new(0.0, 10.0));
        assert_eq!(built.heard(), []);
        built.pulled(265.0);
        assert_eq!(built.heard(), [offset(HEADER_OFFSET_EVENT, false, 0.5)]);
    }

    /// `headerreleased` comes with the lift that leaves the header pulled
    /// at least its height (native's rule; web-core's 90% is not), and
    /// with no other lift.
    #[test]
    fn headerreleased_fires_at_a_lift_with_the_header_fully_pulled() {
        let mut built = Built::listening(&[], &[HEADER_RELEASED_EVENT]);
        built.touch(TOUCH_START);
        built.pulled(245.0);
        built.touch(TOUCH_END);
        assert_eq!(built.heard(), [], "90% is not enough");
        for y in [240.0, 100.0] {
            built.touch(TOUCH_START);
            built.pulled(y);
            assert_eq!(built.heard(), [], "nothing before the lift");
            built.touch(TOUCH_END);
            assert_eq!(built.heard(), [plain(HEADER_RELEASED_EVENT)], "at {y}");
        }
    }

    /// `startrefresh` `{isManual: true}` comes with the `scrollend` that
    /// rests `#container` on the header's snap position (240, or within a
    /// pixel of it), once: another rest there adds nothing until a rest on
    /// `#content` ends the refresh and a new pull starts one.
    #[test]
    fn startrefresh_fires_once_per_rest_on_the_header() {
        let mut built = Built::listening(&[], &[START_REFRESH_EVENT]);
        built.rests(250.0);
        assert_eq!(built.heard(), [], "the header is not at its snap position");
        built.rests(240.0);
        assert_eq!(built.heard(), [start_refresh(true)]);
        built.rests(240.0);
        built.rests(239.5);
        assert_eq!(built.heard(), [], "the same refresh");
        built.rests(290.0);
        assert_eq!(built.heard(), [], "the rest on `#content` starts nothing");
        built.rests(240.75);
        assert_eq!(
            built.heard(),
            [start_refresh(true)],
            "a new pull, rounded within the pixel"
        );
        built.rests(290.0);
        built.rests(241.5);
        assert_eq!(built.heard(), [], "outside the pixel");
    }

    /// `autoStartRefresh` queues `startrefresh` `{isManual: false}` at once,
    /// and the rest on the header that follows adds nothing. A second call
    /// while the refresh runs fires nothing, and the next manual pull,
    /// after a rest on `#content`, is manual again.
    #[test]
    fn auto_start_refresh_fires_startrefresh_at_once_not_manual() {
        let mut built = Built::listening(&[], &[START_REFRESH_EVENT]);
        assert_eq!(built.invoke("autoStartRefresh", "{}"), MethodOutcome::Done);
        assert_eq!(built.heard(), [start_refresh(false)]);
        built.rests(240.0);
        assert_eq!(built.heard(), [], "the landing repeats nothing");
        assert_eq!(built.invoke("autoStartRefresh", "{}"), MethodOutcome::Done);
        assert_eq!(built.heard(), [], "the refresh is running");
        built.rests(290.0);
        built.rests(240.0);
        assert_eq!(built.heard(), [start_refresh(true)], "manual again");
    }

    /// `finishRefresh` ends the refresh at once: the next rest on the
    /// header starts one, even with no rest on `#content` between, and so
    /// does `autoStartRefresh`. `finishLoadMore` does not end it.
    #[test]
    fn finish_refresh_ends_the_refresh() {
        let mut built = Built::listening(&[], &[START_REFRESH_EVENT]);
        built.rests(240.0);
        assert_eq!(built.heard(), [start_refresh(true)]);
        assert_eq!(built.invoke("finishLoadMore", "{}"), MethodOutcome::Done);
        built.rests(240.0);
        assert_eq!(built.heard(), [], "a load's end is not the refresh's");
        assert_eq!(built.invoke("finishRefresh", "{}"), MethodOutcome::Done);
        built.rests(240.0);
        assert_eq!(built.heard(), [start_refresh(true)]);
        assert_eq!(built.invoke("finishRefresh", "{}"), MethodOutcome::Done);
        assert_eq!(built.invoke("autoStartRefresh", "{}"), MethodOutcome::Done);
        assert_eq!(built.heard(), [start_refresh(false)]);
    }

    /// `footeroffset` on each `scroll` that leaves the footer showing,
    /// measured from the scrollport's end: 0.5 halfway in, 1 with its end
    /// on the scrollport's end (340), 5.8 at the range's end with the whole
    /// bottom placeholder showing.
    #[test]
    fn footeroffset_reports_how_far_the_footer_is_pushed() {
        let mut built = Built::listening(&[], &[FOOTER_OFFSET_EVENT]);
        built.pulled(290.0);
        assert_eq!(built.heard(), []);
        built.touch(TOUCH_START);
        for (y, percent) in [(315.0, 0.5), (340.0, 1.0), (580.0, 5.8)] {
            built.pulled(y);
            assert_eq!(
                built.heard(),
                [offset(FOOTER_OFFSET_EVENT, true, percent)],
                "at {y}"
            );
        }
        built.touch(TOUCH_END);
        built.pulled(330.0);
        assert_eq!(built.heard(), [offset(FOOTER_OFFSET_EVENT, false, 0.8)]);
    }

    /// `footerreleased` at a lift with the footer pushed at least its
    /// height; `startloadmore` at the rest on the footer's snap position
    /// (340), once until a rest elsewhere or `finishLoadMore` ends the load.
    #[test]
    fn footerreleased_at_the_lift_and_startloadmore_at_the_rest() {
        let mut built = Built::listening(&[], &[FOOTER_RELEASED_EVENT, START_LOAD_MORE_EVENT]);
        built.touch(TOUCH_START);
        built.pulled(330.0);
        built.touch(TOUCH_END);
        assert_eq!(built.heard(), [], "80% is not enough");
        built.touch(TOUCH_START);
        built.pulled(345.0);
        built.touch(TOUCH_END);
        assert_eq!(built.heard(), [plain(FOOTER_RELEASED_EVENT)]);
        built.rests(340.0);
        assert_eq!(built.heard(), [plain(START_LOAD_MORE_EVENT)]);
        built.rests(340.0);
        assert_eq!(built.heard(), [], "the same load");
        built.rests(290.0);
        built.rests(339.5);
        assert_eq!(
            built.heard(),
            [plain(START_LOAD_MORE_EVENT)],
            "a new push, within the pixel"
        );
        assert_eq!(built.invoke("finishLoadMore", "{}"), MethodOutcome::Done);
        built.rests(340.0);
        assert_eq!(built.heard(), [plain(START_LOAD_MORE_EVENT)]);
    }

    /// Under `enable-auto-loadmore="true"` a lift with any of the footer
    /// showing starts the load, and the rest on the footer that may follow
    /// adds nothing; the rest on `#content` a short push glides back to
    /// ends it. Any other value waits for the rest.
    #[test]
    fn enable_auto_loadmore_starts_the_load_at_the_lift() {
        let mut built = Built::listening(
            &[("enable-auto-loadmore", "true")],
            &[FOOTER_RELEASED_EVENT, START_LOAD_MORE_EVENT],
        );
        built.touch(TOUCH_START);
        built.pulled(300.0);
        built.touch(TOUCH_END);
        assert_eq!(built.heard(), [plain(START_LOAD_MORE_EVENT)]);
        built.rests(290.0);
        assert_eq!(built.heard(), []);
        built.touch(TOUCH_START);
        built.pulled(340.0);
        built.touch(TOUCH_END);
        assert_eq!(
            built.heard(),
            [plain(FOOTER_RELEASED_EVENT), plain(START_LOAD_MORE_EVENT)]
        );
        built.rests(340.0);
        assert_eq!(built.heard(), [], "the rest repeats nothing");

        for value in ["", "false", "TRUE"] {
            let mut built =
                Built::listening(&[("enable-auto-loadmore", value)], &[START_LOAD_MORE_EVENT]);
            built.touch(TOUCH_START);
            built.pulled(340.0);
            built.touch(TOUCH_END);
            assert_eq!(built.heard(), [], "{value:?}");
            built.rests(340.0);
            assert_eq!(built.heard(), [plain(START_LOAD_MORE_EVENT)], "{value:?}");
        }
    }

    /// `enable-refresh="false"` gives the header no box: no header event
    /// fires, whatever moves, and `autoStartRefresh` fires nothing either,
    /// while the footer, 10px into the scrollport, still reports.
    /// `enable-loadmore="false"` does the same for the footer.
    #[test]
    fn an_end_turned_off_fires_nothing() {
        let mut built = Built::listening(&[("enable-refresh", "false")], &EVERY_EVENT);
        assert_eq!(built.offset(), Vector2D::zero(), "opens on `#content`");
        built.touch(TOUCH_START);
        built.pulled(10.0);
        assert_eq!(built.heard(), [offset(FOOTER_OFFSET_EVENT, true, 0.2)]);
        built.pulled(0.0);
        built.touch(TOUCH_END);
        built.rests(0.0);
        assert_eq!(built.invoke("autoStartRefresh", "{}"), MethodOutcome::Done);
        assert_eq!(built.heard(), []);

        let mut built = Built::listening(&[("enable-loadmore", "false")], &EVERY_EVENT);
        built.touch(TOUCH_START);
        built.pulled(290.0);
        built.touch(TOUCH_END);
        built.rests(290.0);
        assert_eq!(built.heard(), [], "the range ends on `#content`");
        built.rests(240.0);
        assert_eq!(
            built.heard(),
            [start_refresh(true)],
            "the header still works"
        );
    }
}
