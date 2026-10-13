//! The `x-swiper` and `x-swiper-item` tags: a row (or, with `vertical`, a
//! column) of items the user swipes through one item at a time, in one of
//! five layouts, with a strip of dots, the autoplay that turns it on a timer,
//! and the `scrollTo` UI method that turns to an item from script.
//!
//! Translated from web-elements'
//! `lynx-stack/packages/web-platform/web-elements/src/elements/XSwiper/x-swiper.css`
//! and its template (`htmlTemplates.ts:225-275`). Only web-core's two tag
//! names exist: native registers `swiper` (`@LynxBehavior(tagName = "swiper")`,
//! Android `XSwiperUI.java:52`), but web-core's tag map has no entry for it
//! (`web-core/ts/constants.ts`), so a compiled card that reaches web-core
//! writes `x-swiper` and `x-swiper-item`.
//!
//! # A component with a shadow tree
//!
//! `x-swiper` is the first tag here with a UA shadow tree. Its component
//! ([`Swiper`]) attaches one when the element is constructed, holding exactly
//! two elements, in web-core's roles:
//!
//! - `#content`, the scroll container: a row (a column under `vertical`) flex box of the host's
//!   size, holding a default `<slot>` (`display: contents`) the light-DOM items are assigned to. It
//!   scrolls, snaps `mandatory` on its main axis (css-scroll-snap-1), and the painter's snap rules
//!   settle every release on an item (`crates/bobcat-core/src/paint/inertia.rs`).
//! - `#indicator`, the dot strip, below.
//!
//! web-core's `#bounce-padding` and its two circular slots are left out. The
//! shadow sheet ([`SHADOW_RULES`]) is added per instance and reaches the host
//! through `:host(…)`; the light-DOM item rules stay in the UA sheet
//! ([`UA_RULES`]) and match the items as the host's children, while their
//! percentages resolve against `#content`, their flat-tree parent, which is the
//! host's size. The host keeps the container defaults every tag gets
//! ([`super::ua_sheet`]): its `display` follows `defaultDisplayLinear`, it
//! clips, and it is the strip's containing block. `--swiper-current`,
//! `--swiper-count` and the two colours are declared on the host and inherited
//! through the flat tree. The shadow tree is out of every node-tree path the
//! realm reads — a host's children are its items, an item's parent is the host,
//! and selector queries do not cross into the shadow tree.
//!
//! # The main axis cannot be moved
//!
//! The axis lives on `#content`, which no author rule reaches, as in web-core:
//! `flex-direction`, `flex-wrap: nowrap` and `justify-content` are plain
//! declarations in the shadow sheet. `#content` is a flex box whatever the
//! host's `display`, so `linear-direction` has no rule. An item's main-axis
//! size is web-core's own `!important` (`x-swiper.css:102-118` and every
//! mode's size), and stays so: an item an author sizes would stop filling its
//! page. `docs/style-assumptions.md` §D.15 records them; [`super::ua_sheet`]'s
//! pinned test lists them. An item's `position` is not pinned, as web-core
//! does not pin it.
//!
//! # Boolean attributes and `"false"`
//!
//! web-core removes an attribute whose value is `"false"` before its CSS and
//! its code see it, except the ones a component lists in
//! `notToFilterFalseAttributes` — for the swiper `smooth-scroll` and
//! `indicator-dots` (`XSwiper.ts:28-31`). This engine's `__SetAttribute`
//! stringifies `false`, so `vertical`, `bounces` and `circular` match as
//! "present and not `"false"`", and `indicator-dots` is read below as web-core
//! reads it.
//!
//! # What the attributes do
//!
//! - `vertical`: the column. Overflow axes swap, `scroll-snap-type: y mandatory`
//!   (`x-swiper.css:55-62`).
//! - `mode`: `normal` (default) — items 100% of the main axis, start-aligned. `carousel` — items
//!   80%, start-aligned, the last item followed by a 20% margin so it can reach its start
//!   (`:181-195`). `flat-coverflow` — items 60%, centre-aligned, 20% margin before the first and
//!   after the last (`:271-297`). `coverflow` — `flat-coverflow`'s geometry plus a scroll-driven
//!   turn (`:205-267`). `carry` — items 100%, centre-aligned, plus a scroll-driven scale
//!   (`:299-317`). Each margin sits on the main axis; web-core writes the carousel's as
//!   `margin-right` in both orientations (`:193-195`), here it is `margin-bottom` under `vertical`.
//!   Under `circular` there is no margin (web-core's margin rules all carry `:not([circular])`,
//!   `:193`, `:235-250`, `:280-297`): the items pack edge to edge, so the seam the painter draws
//!   joins the last item to the first, not to the swiper's own background. A percentage margin
//!   resolves against `#content`'s width on every side, as CSS resolves it, so under `vertical` the
//!   20% is of the width, as in web-core; native offsets by 20% of the main axis
//!   (`XSwiperUI.java:582-604`).
//! - `current`: the item the swiper starts on. See below.
//! - `bounces` (present and not `"false"`): `overscroll-behavior` `contain-bounce` on `#content`'s
//!   main axis, as for [`super::viewpager`]. web-core shows a page-wide blank box ahead of the
//!   items (`x-swiper.css:64-66`, `htmlTemplates.ts:226-232`), so only the leading edge can be
//!   pulled.
//! - `circular` (present and not `"false"`): `overscroll-behavior: circular` on `#content`'s main
//!   axis, so the axis wraps: the painter keeps its offset on a circle one scrolling area long,
//!   draws the seam, and its periodic snap rules page across it, so a drag or flick past the last
//!   item lands on the first and back. Its rules come after `bounces`' and win on the same axis: a
//!   circular axis has no edge to bounce. The mode margins are off (above), so the period is the
//!   items' extent alone. Without them an end item's snap position can lie past `max_offset` (the
//!   last `carousel` item's start) or before `0` (the first centred item's centre); the document
//!   admits the whole period on a circular axis, so such a position is one across the seam — the
//!   first `flat-coverflow` item centred stands at the period less its offset — and `current` and
//!   `scrollTo({index})` land on the item in every mode. web-core re-slots the first and last items
//!   into its shadow tree around the current one (`XSwiperCircular.ts`) and turns snapping off
//!   (`x-swiper.css:122-131`); native loops its `ViewPager` (`XSwiperUI.java:865-868`, `setLoop`).
//!   Here snapping stays. `docs/tracking/deviations.md` records the difference.
//! - `indicator-dots`, `indicator-color`, `indicator-active-color`: the strip, below.
//!
//! A child of the swiper that is neither an `x-swiper-item` nor a `wrapper` generates no box
//! (`x-swiper.css:98-100`; web-core's `lynx-wrapper` keeps `display: contents !important`). An item
//! gets the item rules only as the swiper's own child: one inside a `wrapper` is an ordinary
//! container, where web-core, which also sizes only the swiper's own children, still snaps it
//! (`:68-75`).
//!
//! # The last item's end margin is scroll range
//!
//! A scroll container's scrollable overflow takes in its flex items' margin areas (css-overflow-3
//! §3.3, `hughie`'s `item_end_margin`), so the 20% after the last item extends the range: a
//! horizontal `carousel` scrolls its last item to its start, and a horizontal `flat-coverflow` or
//! `coverflow` centres it. Under `vertical` the margin is 20% of the width, so a last item whose
//! main axis is longer than its width stops short by the difference, as it does in a browser
//! running web-core's rules. Under `circular` there is no margin and no end to the range: the
//! last item's position is admitted on the circle (above).
//!
//! # The two animated modes are scroll-driven, and 2D
//!
//! `coverflow` and `carry` transform each item by where it stands in the scrollport:
//! `animation-timeline: view(inline)` (`view(block)` under `vertical`; this engine's `inline` is
//! x), `animation-duration: auto`, `animation-fill-mode: both`, linear timing. `carry` runs `0%`
//! `scale(0.6)`, `45%, 55%` `scale(1)`, `100%` `scale(0.6)`.
//!
//! web-core's `coverflow` turns each item about the axis across the swiper's main one — `rotateY`,
//! `rotateX` under `vertical` — by up to 50° with `scale(0.8)`, under `perspective: 200px` on
//! `#content` (`x-swiper.css:205-267`). A rotation under a perspective is no 2D affine, and a 2D
//! affine is all the painter composes a curve with: such a keyframe refuses the curve export
//! (`TransformTrack::new`, `crates/dom/src/visual/curves.rs`), and then every item would
//! re-cascade on the main thread at every scroll frame. `coverflow` here draws the same turn in a
//! parallel projection, which is a 2D affine, as three functions every keyframe lists in the same
//! order, so each interpolates on its own and no sample leaves the plane:
//!
//! - `scale(0.514, 0.8)` (`scale(0.8, 0.514)` under `vertical`): web-core's `scale(0.8)`, and on
//!   the main axis the width a turned item shows, `0.8 × cos 50°`.
//! - `skewY(14deg)` (`skewX`): the depth cue the perspective gave. The projection looks at a row
//!   from above, so an item's outer edge, the one nearer the viewer in web-core's turn, drops and
//!   its inner edge rises; a column is the row transposed. About its centre, so the item keeps its
//!   place on the cross axis. The sign flips with the side.
//! - `translateX(-24.3%)` (`translateY`): half of the width the scale removed, `(1 − 0.514) / 2`,
//!   toward the current item. The edge of the item that faces the current one therefore stays where
//!   layout put it at every sample, and the item gives up width at its outer edge alone. Two
//!   consequences: no item ever overlaps its neighbour — paint order is tree order here, because
//!   web-core's `z-index: 1` keyframe has no painter curve — and the item beside the current one
//!   keeps the 20% of the scrollport layout gave it, where a scale about its centre would leave
//!   under 10%. The sign flips with the side.
//!
//! The stops are `0%` and `100%` turned and `45%, 55%` flat, so an item is turned the more the
//! farther it stands from the middle. web-core's are `25%` (−50°), `45%, 55%` and `100%` (40°)
//! with nothing at `0%`, so an item entering at the end edge starts flat, turns, and turns back.
//! At rest the two items beside the current one stand at 12.5% and 87.5% of their timelines — a
//! 60% item in a 100% scrollport — which is 72% of the way to the full pose: `scale(0.649,
//! 0.856)`, the width of a 41° turn, and `skewY(10.1deg)`. web-core's `ease` segments leave them
//! at −40° and 38° there.
//!
//! Both modes export: the painter samples them from `#content`'s live offset, with no main-thread
//! tick and no commit while the user drags. `fill-mode: both` is what lets them export at any
//! committed offset (`docs/tracking/css-animation.md`, "Base-value rule"). web-core leaves the
//! timing at `ease` per segment; linear is this engine's choice, and is what keeps the inner edge
//! still: the translation stays half of what the scale removed only while the two interpolate at
//! the same rate.
//!
//! What a parallel projection cannot show is the perspective's convergence: a turned item is a
//! parallelogram, not a trapezoid, its two cross-axis edges the same length. The skew moves an
//! item's corners along the cross axis: at rest by 5.8% of the item's main-axis size (`tan 10.1°
//! × 0.649 / 2`), against the 7.2% of its cross-axis size the scale freed. On a swiper more than
//! 2.1 times as long as it is across, a corner therefore passes the swiper's box, where `#content`
//! clips it, as it clips the edge web-core's perspective enlarges.
//!
//! # `current` is CSS
//!
//! The [`super::viewpager`] recipe with a property of its own: the swiper sets the registered,
//! inherited `<integer>` `--swiper-current` from `attr(current type(<integer>), -1)`, and each
//! direct item is its container's `scroll-initial-target` exactly when that number equals its
//! `sibling-index() - 1` (`docs/style-assumptions.md` §27). The container `nearest` finds, over
//! the flat tree, is `#content`. The initial target scrolls the item into view (`inline:
//! nearest`, `block: start`) and the painter's at-rest snap rule settles it on the item's snap
//! position — its start, or its centre in `flat-coverflow`, `coverflow` and `carry` — which is
//! where web-core lands too. Under `circular` that position may lie past `max_offset` or before
//! `0`; the document admits it on the circle, so the swiper still starts on the item. web-core's
//! `#scrollToIndex` (`XSwiper.ts:104-129`) aims at the centre in
//! `flat-coverflow` and at the start elsewhere, and mandatory snapping moves a start in the other
//! centred modes to the centre. web-core reads `current` at connect (`:172-178`) and on every
//! change (`XSwiperAutoScroll.ts:35-41`); a change here turns the swiper instantly where web-core
//! turns it smoothly. Every other consequence of a target that is honoured once is the pager's,
//! listed in `docs/tracking/deviations.md`.
//!
//! # The dot strip
//!
//! `#indicator` is one absolutely positioned box with no children: the dots are its two
//! background layers, not boxes. web-core builds one `<div>` per item into its
//! `#indicator-container` (`XSwiperIndicator.ts:76-102`); here the count is a number the cascade
//! reads instead. [`Swiper`] keeps the registered `<integer>` `--swiper-count` on the host equal to
//! the number of elements assigned to its slot — every element child, since the one slot is the
//! default one, as web-core's `childElementCount` counts them — through a presentational hint it
//! rewrites on the standard's `slotchange` at that slot, heard through
//! [`CustomElement::handle_event`](dom::CustomElement::handle_event) where a script component
//! listens on its shadow root. web-core observes the host's child list with a `MutationObserver`
//! for the same purpose (`:103-113`).
//!
//! - **Geometry** (`x-swiper.css:137-178`): a dot is `--indicator-size: 0.6rem` across and the
//!   pitch is `size × 7 / 5` — the dot plus web-core's `size / 5` margin on each side. The strip is
//!   `count × pitch` long and one dot across, centred on the main axis with `left: 50%` and
//!   `translateX(-50%)` and `0.5rem` in from the bottom edge (`translateY(-50%)`, `top: 50%`, and
//!   the right edge under `vertical`). web-core's container spans the whole edge and centres its
//!   dots in it; this box spans only the dots. `z-index: 100` is web-core's; `pointer-events` is
//!   untouched, so a press on the strip lands on it, as on web-core's container.
//! - **Colours**: the host declares `--indicator-color: attr(indicator-color type(<color>),
//!   #ffffff4d)` and `--indicator-active-color: attr(indicator-active-color type(<color>), white)`,
//!   web-core's defaults; a value that is no colour takes the default.
//! - **Inactive dots**: the bottom layer, a filled circle (`radial-gradient(circle closest-side,
//!   …)`) in a `pitch × size` tile repeated along the strip. Its stops are `0%`, `95%`, `105%`: the
//!   painter's gradient ramp reads a first stop as offset 0 whatever its position, and the fork's
//!   gradient grammar takes percentages only, so a hard `100%` edge would draw as a fade.
//! - **The active dot**: the top layer, the same circle in the active colour, not repeated, whose
//!   `background-position-x` (`-y` under `vertical`) a scroll-driven animation moves: `#content`
//!   names its scroll timeline `--swiper-scroller`, the host's `timeline-scope` (declared by the
//!   shadow sheet's `:host`, so the name, the scope and the reference share one tree) lets
//!   `#indicator`, `#content`'s sibling, find it, and the keyframes run the layer from `0` to
//!   `(count − 1) × pitch` under `steps(count, jump-none)`. At rest on item `k` the timeline's
//!   progress is `k / (count − 1)` — the scroll range is `(count − 1)` snap steps in every mode,
//!   because the last item's end margin is scroll range (above) — which lies in the `k`-th of the
//!   `count` steps, whose held value is `k × pitch`. Under `circular` there is no end margin and an
//!   end item at rest may stand across the seam: the timeline reads an offset past `max_offset` at
//!   its representative nearer the range (`dom::ScrollBox::unwrapped_offset`), so the first item
//!   centred just before the start shows the first dot and a last item past `max_offset` the last.
//!   An item between them reads its own offset over `max_offset`, which is not `k / (count − 1)`
//!   there and need not lie in the `k`-th step once there are five items or more. web-core instead
//!   runs one animation per dot on that item's view timeline, active between 30% and 70% of it
//!   (`htmlTemplates.ts:253-266`).
//! - **Cost**: `background-position` exports to no painter curve, so the strip re-cascades on the
//!   main thread after each scroll the main thread adopts — one element per swiper. A `"false"`
//!   strip drops its `animation-name` as well as its box: `display: none` alone does not stop an
//!   animation in this engine (Stylo's Servo half starts animations on a `display: none` element).
//! - **Polarity**: shown by default and by `"true"`, hidden by any other present value. The default
//!   is web-core's: its rule `x-swiper[indicator-dots]::part(indicator-container) { display: none
//!   }` (`x-swiper.css:133-135`) hides the dots for any present value, `"true"` included, and its
//!   goldens show dots by default and hide them for `"false"` and for the literal string
//!   `"{{false}}"` (`basic-element-x-swiper-indicator-dots`). `"true"` showing is native's, which
//!   hides them by default and shows them for `true` alone (`XSwiperUI.java:887-890`). Every value
//!   but `"true"` therefore agrees with both references, and `"true"` with native.
//!   `docs/tracking/deviations.md` records each side.
//!
//! # `scrollTo` is the standard algorithm
//!
//! The component's one UI method is web-core's `scrollTo({index, smooth = true})` overload
//! (`XSwiper.ts:142-170`), [`scroll_to`], which the runtime's `callElementMethod` reaches through
//! [`dom::Document::invoke_element_method`]. It runs CSSOM-View's "scroll an element into view"
//! ([`dom::Document::scroll_into_view`]) on the swiper's element child number `index`, aligned on
//! the main axis as the item's own `scroll-snap-align` aligns it, in `#content`. That is the
//! item's snap position, which is where web-core's per-mode offsets (`XSwiper.ts:104-129`) end up
//! after mandatory snapping, in every mode. A bad or out-of-range `index` is code 4, Android's
//! answer, where web-core does nothing and answers 0; Android's `direction` is not read.
//!
//! # Autoplay is a timer of the component's
//!
//! `autoplay`, `interval` and `smooth-scroll` are web-core's `XSwiperAutoScroll.ts`, run by the
//! component rather than by script. A connected swiper that has `autoplay` keeps one timer in the
//! document's [`Autoplays`], which the page waits out with its realm's own timers and runs in its
//! epilogue ([`Autoplays::run_due`]); every `interval` milliseconds (5000 when it is missing or
//! not a number) it turns to the next item ([`turn`]). Nothing is armed, read or waited for on a
//! page with no such swiper.
//!
//! When the timer starts and stops is web-core's, to the letter (user ruling 2026-10-13):
//!
//! - It starts when the swiper is connected with `autoplay`, and starts over whenever `autoplay` or
//!   `interval` changes on a connected swiper that has `autoplay` (`XSwiperAutoScroll.ts:60-70`,
//!   after web-core's connect pass, `component.ts:226-258`). The first turn is one whole interval
//!   later.
//! - **Removing `autoplay` does not stop it.** web-core's handler has no branch for the attribute
//!   going away and its tick never rechecks it. Android cancels (`XSwiperUI.java:684-691`).
//!   `autoplay="false"` is the attribute removed, as web-core's false-filtering makes it.
//! - Only disconnection ends it (`:72-74`); reconnecting with `autoplay` starts a new one.
//!
//! A turn that comes due while a finger holds the swiper is not carried out, and the timer goes
//! on unchanged (user ruling 2026-10-13): the turn is a scroll request the painter declines while
//! a pointer has the container ([`dom::Document::scroll_to_yielding`]). web-core scrolls under
//! the finger; Android stops its timer for the drag and waits a whole interval after it
//! (`XSwiperUI.java:208-220`).
//!
//! # What is not implemented
//!
//! - Android's `finish-reset`, which rewinds a swiper that is not `circular` from its last item
//!   (`XSwiperUI.java:693-696`): web-core has none.
//! - The perspective of `coverflow`: its turn is drawn in a parallel projection, above.
//! - `page-margin`, `previous-margin`, `next-margin` and `duration` have no rule, following
//!   web-core, where they change nothing on screen: the three margins become custom properties on
//!   the indicator container (`XSwiperIndicator.ts:54-74`), so the items always see `0px`, and
//!   nothing observes `duration`. Native differs: `page-margin` is the gap between pages
//!   (`XSwiperUI.java:569`), `previous-margin`/`next-margin` size and offset the page in
//!   `coverflow`, `flat-coverflow` and `carry` (`:617-638`), and `duration` is the turn's animation
//!   length, 500 ms by default (`:91,855-863`).
//! - Every event (`change`, `scrollstart`, `scrollend`, `transition`).
//! - `contain: strict` and `content-visibility: auto` from the twentieth item
//!   (`x-swiper.css:77-80`), `scrollbar-width` and the per-item `view-timeline-name` rules: a
//!   browser shortcut, a scrollbar this engine does not draw, and web-core's per-dot timelines.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use dom::event::{ElementEvent, ElementEventKind, EventPhase};
use dom::scroll::{
    ScrollBehavior, ScrollIntoViewContainer, ScrollIntoViewOptions, ScrollLogicalPosition,
};
use dom::stylo::values::specified::box_::ScrollSnapAlignKeyword;
use dom::{
    CustomElement, MethodCall, MethodError, MethodOutcome, Node, NodeId, ShadowRootMode, Vector2D,
};
use serde_json::Value;
use smallvec::SmallVec;

use super::{InvalidParams, LynxDocument, element_child, is_truthy};
use crate::clock::ClockInstant;

/// The swiper tag, web-core's only spelling of it.
pub(super) const SWIPER_TAG: &str = "x-swiper";
/// The item tag, web-core's only spelling of it.
pub(super) const SWIPER_ITEM_TAG: &str = "x-swiper-item";

/// The `x-swiper` policy outside the shadow tree — the host's custom
/// properties and the light-DOM item rules — in `x-swiper.css`'s own order;
/// see the module documentation for what each rule is for.
///
/// `[vertical]:not([vertical="false"])` is the column and
/// `:is(:not([vertical]), [vertical="false"])` the row. Each important
/// declaration sits on a line of its own, because [`super::ua_sheet`]'s
/// pinned test reads the sheet line by line. `@property` rules live here, not
/// in [`SHADOW_RULES`]: a registration is document-wide.
pub(super) const UA_RULES: &str = r#"
@property --swiper-current { syntax: "<integer>"; inherits: true; initial-value: -1; }
@property --swiper-count { syntax: "<integer>"; inherits: true; initial-value: 0; }
x-swiper {
  --swiper-current: attr(current type(<integer>), -1);
  --indicator-color: attr(indicator-color type(<color>), #ffffff4d);
  --indicator-active-color: attr(indicator-active-color type(<color>), white);
}
x-swiper > x-swiper-item {
  height: 100%; flex: 0 0 auto; scroll-snap-align: start;
  scroll-initial-target:
    if(style(--swiper-current: calc(sibling-index() - 1)): nearest; else: none);
}
x-swiper > :not(x-swiper-item, wrapper) { display: none; }
x-swiper > x-swiper-item { width: 100% !important; }
x-swiper[vertical]:not([vertical="false"]) > x-swiper-item { height: 100% !important; }
x-swiper[mode="carousel"]:is(:not([vertical]), [vertical="false"]) > x-swiper-item { width: 80% !important; }
x-swiper[mode="carousel"][vertical]:not([vertical="false"]) > x-swiper-item { height: 80% !important; }
x-swiper:is([mode="flat-coverflow"], [mode="coverflow"]):is(:not([vertical]), [vertical="false"]) > x-swiper-item { width: 60% !important; }
x-swiper:is([mode="flat-coverflow"], [mode="coverflow"])[vertical]:not([vertical="false"]) > x-swiper-item { height: 60% !important; }
x-swiper:is([mode="flat-coverflow"], [mode="coverflow"]):is(:not([circular]), [circular="false"]):is(:not([vertical]), [vertical="false"])
  > x-swiper-item:first-child { margin-left: 20%; }
x-swiper:is([mode="flat-coverflow"], [mode="coverflow"]):is(:not([circular]), [circular="false"])[vertical]:not([vertical="false"])
  > x-swiper-item:first-child { margin-top: 20%; }
x-swiper:is([mode="carousel"], [mode="flat-coverflow"], [mode="coverflow"]):is(:not([circular]), [circular="false"]):is(:not([vertical]), [vertical="false"])
  > x-swiper-item:last-child { margin-right: 20%; }
x-swiper:is([mode="carousel"], [mode="flat-coverflow"], [mode="coverflow"]):is(:not([circular]), [circular="false"])[vertical]:not([vertical="false"])
  > x-swiper-item:last-child { margin-bottom: 20%; }
x-swiper:is([mode="flat-coverflow"], [mode="coverflow"], [mode="carry"]) > x-swiper-item {
  scroll-snap-align: center;
}
x-swiper:is([mode="coverflow"], [mode="carry"]) > x-swiper-item {
  animation-duration: auto; animation-timing-function: linear; animation-fill-mode: both;
  animation-timeline: view(inline);
}
x-swiper:is([mode="coverflow"], [mode="carry"])[vertical]:not([vertical="false"]) > x-swiper-item {
  animation-timeline: view(block);
}
x-swiper[mode="coverflow"] > x-swiper-item { animation-name: x-swiper-coverflow-horizontal; }
x-swiper[mode="coverflow"][vertical]:not([vertical="false"]) > x-swiper-item {
  animation-name: x-swiper-coverflow-vertical;
}
x-swiper[mode="carry"] > x-swiper-item { animation-name: x-swiper-carry; }
@keyframes x-swiper-coverflow-horizontal {
  0% { transform: translateX(-24.3%) skewY(14deg) scale(0.514, 0.8); }
  45%, 55% { transform: translateX(0%) skewY(0deg) scale(1, 1); }
  100% { transform: translateX(24.3%) skewY(-14deg) scale(0.514, 0.8); }
}
@keyframes x-swiper-coverflow-vertical {
  0% { transform: translateY(-24.3%) skewX(14deg) scale(0.8, 0.514); }
  45%, 55% { transform: translateY(0%) skewX(0deg) scale(1, 1); }
  100% { transform: translateY(24.3%) skewX(-14deg) scale(0.8, 0.514); }
}
@keyframes x-swiper-carry {
  0% { transform: scale(0.6); }
  45%, 55% { transform: scale(1); }
  100% { transform: scale(0.6); }
}
"#;

/// The swiper's shadow sheet, added to each instance's shadow root: see the
/// module documentation. `[vertical]:not([vertical="false"])` is the column.
const SHADOW_RULES: &str = r#"
:host { timeline-scope: --swiper-scroller; }
slot { display: contents; }
#content {
  display: flex; flex-direction: row; flex-wrap: nowrap; justify-content: flex-start;
  width: 100%; height: 100%; contain: content;
  overflow-x: scroll; overflow-y: clip;
  scroll-snap-type: x mandatory;
  scroll-timeline: --swiper-scroller inline;
}
:host([vertical]:not([vertical="false"])) #content {
  flex-direction: column; overflow-x: clip; overflow-y: scroll; scroll-snap-type: y mandatory;
  scroll-timeline-axis: block;
}
:host([bounces]:not([bounces="false"])) #content { overscroll-behavior-x: contain-bounce; }
:host([vertical]:not([vertical="false"])[bounces]:not([bounces="false"])) #content {
  overscroll-behavior-x: auto; overscroll-behavior-y: contain-bounce;
}
:host([circular]:not([circular="false"])) #content { overscroll-behavior-x: circular; }
:host([vertical]:not([vertical="false"])[circular]:not([circular="false"])) #content {
  overscroll-behavior-x: auto; overscroll-behavior-y: circular;
}
#indicator {
  --indicator-size: 0.6rem;
  --indicator-pitch: calc(var(--indicator-size) * 7 / 5);
  display: flex; position: absolute; z-index: 100;
  left: 50%; bottom: 0.5rem; transform: translateX(-50%);
  width: calc(var(--swiper-count) * var(--indicator-pitch)); height: var(--indicator-size);
  background-image:
    radial-gradient(circle closest-side,
      var(--indicator-active-color) 0%, var(--indicator-active-color) 95%, transparent 105%),
    radial-gradient(circle closest-side,
      var(--indicator-color) 0%, var(--indicator-color) 95%, transparent 105%);
  background-size: var(--indicator-pitch) var(--indicator-size);
  background-repeat: no-repeat, repeat-x;
  background-position: 0 0, 0 0;
  animation: swiper-active-dot auto linear both;
  animation-timeline: --swiper-scroller;
  animation-timing-function: steps(var(--swiper-count), jump-none);
}
:host([vertical]:not([vertical="false"])) #indicator {
  left: auto; bottom: auto; top: 50%; right: 0.5rem; transform: translateY(-50%);
  width: var(--indicator-size); height: calc(var(--swiper-count) * var(--indicator-pitch));
  background-size: var(--indicator-size) var(--indicator-pitch);
  background-repeat: no-repeat, repeat-y;
  animation-name: swiper-active-dot-vertical;
}
:host([indicator-dots]:not([indicator-dots="true"])) #indicator { display: none; animation-name: none; }
@keyframes swiper-active-dot {
  from { background-position-x: 0, 0; }
  to { background-position-x: calc((var(--swiper-count) - 1) * var(--indicator-pitch)), 0; }
}
@keyframes swiper-active-dot-vertical {
  from { background-position-y: 0, 0; }
  to { background-position-y: calc((var(--swiper-count) - 1) * var(--indicator-pitch)), 0; }
}
"#;

/// The scroll container's id in the shadow tree.
const CONTENT_ID: &str = "content";
/// The dot strip's id in the shadow tree.
const INDICATOR_ID: &str = "indicator";
/// The registered custom property the host carries its item count in.
const COUNT_PROPERTY: &str = "--swiper-count";

/// Installs the `x-swiper` component. Must run before any element could
/// carry the tag, which is [`Document::define`](dom::Document::define)'s own
/// precondition.
pub(super) fn define(document: &mut LynxDocument, autoplays: Autoplays) {
    document.define(SWIPER_TAG, Box::new(Swiper { autoplays }));
}

/// The period of an autoplay with no usable `interval`, web-core's
/// (`XSwiperAutoScroll.ts:65-68`) and Android's (`XSwiperUI.java:90`).
const DEFAULT_AUTOPLAY_INTERVAL: Duration = Duration::from_secs(5);

/// The least period an autoplay runs at, in milliseconds: the delay HTML
/// clamps a nested timer to, which is what a browser's `setInterval(…, 0)`
/// settles at.
const LEAST_AUTOPLAY_MILLISECONDS: f64 = 4.0;

/// The longest period, in milliseconds: HTML's `long` timeout, about 24
/// days.
const LONGEST_AUTOPLAY_MILLISECONDS: f64 = 2_147_483_647.0;

/// The autoplay timers of one document's swipers: for each connected
/// `x-swiper` that was given `autoplay`, when its next turn is due and how
/// long after that the following one is.
///
/// Shared, as [`super::ComponentEvents`] is, between the component, which
/// starts and stops the timers from its reactions, and the runtime, which
/// waits for the earliest ([`Self::next_deadline`]) and runs what came due
/// ([`Self::run_due`]). Empty on a page with no autoplaying swiper, where
/// each of the two is one test.
#[derive(Clone, Default)]
pub(crate) struct Autoplays(Rc<RefCell<Vec<Autoplay>>>);

struct Autoplay {
    swiper: NodeId,
    period: Duration,
    due: ClockInstant,
}

impl Autoplays {
    /// Starts `swiper`'s timer over: its first turn is `period` after `now`.
    fn start(&self, swiper: NodeId, period: Duration, now: ClockInstant) {
        let mut timers = self.0.borrow_mut();
        let timer = Autoplay {
            swiper,
            period,
            due: now + period,
        };
        match timers.iter_mut().find(|armed| armed.swiper == swiper) {
            Some(armed) => *armed = timer,
            None => timers.push(timer),
        }
    }

    fn stop(&self, swiper: NodeId) {
        self.0.borrow_mut().retain(|armed| armed.swiper != swiper);
    }

    /// When the earliest turn is due, if any swiper autoplays.
    pub(crate) fn next_deadline(&self) -> Option<ClockInstant> {
        self.0.borrow().iter().map(|armed| armed.due).min()
    }

    /// Turns every swiper whose timer has come due at `now` ([`turn`]) and
    /// arms each one's next turn a period after `now`, as `setInterval`
    /// re-arms from the moment it fires.
    pub(crate) fn run_due(&self, document: &mut LynxDocument, now: ClockInstant) {
        let due: SmallVec<[NodeId; 1]> = self
            .0
            .borrow_mut()
            .iter_mut()
            .filter(|armed| armed.due <= now)
            .map(|armed| {
                armed.due = now + armed.period;
                armed.swiper
            })
            .collect();
        // With the list released: a turn mutates the document.
        for swiper in due {
            turn(document, swiper);
        }
    }
}

/// The period `interval` names: web-core's `parseFloat` of the attribute in
/// milliseconds, 5000 when it is missing or no number
/// (`XSwiperAutoScroll.ts:64-68`), within what a timer admits: no shorter
/// than a nested timer's 4 ms, which is where a browser's `setInterval` of 0
/// or of a negative delay settles, and no longer than HTML's longest
/// timeout.
fn autoplay_period(interval: Option<&str>) -> Duration {
    let Some(milliseconds) = interval.and_then(parse_float) else {
        return DEFAULT_AUTOPLAY_INTERVAL;
    };
    let milliseconds =
        milliseconds.clamp(LEAST_AUTOPLAY_MILLISECONDS, LONGEST_AUTOPLAY_MILLISECONDS);
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "clamped just above to a positive range far inside u64"
    )]
    let microseconds = (milliseconds * 1000.0).round() as u64;
    Duration::from_micros(microseconds)
}

/// JavaScript's `parseFloat` of a decimal literal: the longest prefix of
/// `text`, past leading white space, that reads as a number. `None` where
/// `parseFloat` answers `NaN`, and for `Infinity`, which no interval is.
fn parse_float(text: &str) -> Option<f64> {
    let text = text.trim_start();
    let numeric = text
        .find(|character: char| !matches!(character, '0'..='9' | '+' | '-' | '.' | 'e' | 'E'))
        .map_or(text, |end| &text[..end]);
    (1..=numeric.len())
        .rev()
        .find_map(|end| numeric[..end].parse::<f64>().ok())
        .filter(|value| value.is_finite())
}

/// The `x-swiper` component: builds the shadow tree when the element is
/// constructed, keeps `--swiper-count` equal to the number of its element
/// children, runs the autoplay timer, and answers the `scrollTo` UI method
/// ([`scroll_to`]).
///
/// The shadow tree is built in `constructed`, not on connection: the element
/// is new and detached then, and `__CreateElement` mints a swiper before
/// anything is appended to it, so every child arrives as a `slotchange` at
/// the slot.
struct Swiper {
    autoplays: Autoplays,
}

impl Swiper {
    /// web-core's `_handleAutoplay` (`XSwiperAutoScroll.ts:62-70`) for a
    /// connected swiper: with `autoplay`, its timer starts over at the
    /// period `interval` names; without, nothing changes, a running timer
    /// included.
    fn handle_autoplay(&self, document: &LynxDocument, swiper: NodeId) {
        let Some(element) = document.get(swiper) else {
            return;
        };
        if flag(element, "autoplay") {
            self.autoplays.start(
                swiper,
                autoplay_period(element.attribute("interval")),
                ClockInstant::now(),
            );
        }
    }
}

impl CustomElement<()> for Swiper {
    fn observed_attributes(&self) -> Vec<String> {
        vec!["autoplay".to_owned(), "interval".to_owned()]
    }

    fn connected_callback(&self, document: &mut LynxDocument, element: NodeId) {
        self.handle_autoplay(document, element);
    }

    fn disconnected_callback(&self, _document: &LynxDocument, element: NodeId) {
        self.autoplays.stop(element);
    }

    /// `autoplay` or `interval` changed. A swiper that is not connected
    /// reads both when it connects, as web-core's handlers run only then.
    fn attribute_changed_callback(
        &self,
        document: &mut LynxDocument,
        element: NodeId,
        _name: &str,
        _old: Option<&str>,
        _new: Option<&str>,
    ) {
        if document.is_connected(element) {
            self.handle_autoplay(document, element);
        }
    }

    fn constructed(&self, document: &mut LynxDocument, element: NodeId) {
        let shadow = document.attach_shadow(element, ShadowRootMode::Open);
        document.add_shadow_stylesheet(shadow, SHADOW_RULES);
        let content = document.create_element("div", ());
        document.set_id_attribute(content, Some(CONTENT_ID));
        let slot = document.create_element("slot", ());
        document.append_child(content, slot);
        document.append_child(shadow, content);
        let indicator = document.create_element("div", ());
        document.set_id_attribute(indicator, Some(INDICATOR_ID));
        document.append_child(shadow, indicator);
        count_items(document, element);
    }

    /// `slotchange` at the swiper's own slot, delivered at its shadow root,
    /// once per pass: the capture delivery is skipped, the bubble one counts.
    fn handle_event(&self, document: &mut LynxDocument, element: NodeId, event: &mut ElementEvent) {
        if event.kind() == ElementEventKind::SlotChange
            && event.phase() == EventPhase::Bubbling
            && slot(document, element) == Some(event.target())
        {
            count_items(document, element);
        }
    }

    fn invoke(
        &self,
        document: &mut LynxDocument,
        element: NodeId,
        call: MethodCall<'_>,
    ) -> MethodOutcome {
        match call.name {
            "scrollTo" => match scroll_to(document, element, call.params) {
                Ok(()) => MethodOutcome::Done,
                Err(InvalidParams) => MethodOutcome::Failed(MethodError::InvalidParams),
            },
            _ => MethodOutcome::NotFound,
        }
    }
}

/// `scrollTo({index, smooth = true})` on `swiper`, with `params` as the JSON
/// text the realm serialized: CSSOM-View's "scroll an element into view"
/// ([`dom::Document::scroll_into_view`]) on the swiper's element child number
/// `index`, at the item's own snap alignment on the swiper's main axis.
///
/// web-core overrides `HTMLElement.scrollTo` on the swiper
/// (`XSwiper.ts:142-170`, documented at `index.ts:31-33`; lynx-stack#1197
/// replaced `scrollToNext` and `scrollToPrevious` with it): a first argument
/// with a numeric `index` turns to that item, and anything else falls through
/// to the standard method. `scrollTo` is also a name in the base set every
/// element has ([`super::base_methods`]); a kind's
/// [`CustomElement::invoke`] runs first, so this overload wins on a swiper
/// the same way. Android's `scrollTo({index, smooth, direction})` is
/// `XSwiperUI.java:651-680`; iOS and Harmony have no swiper.
///
/// - `index` must be a JSON number; a missing one, or one that is not a number — `NaN` and the
///   infinities serialize as `null` — is code 4 with nothing moved. Android reads a missing one as
///   -1 and answers code 4; web-core falls through to the standard `scrollTo` and answers 0.
/// - A fraction is truncated toward zero, as `children.item(index)`'s `unsigned long` conversion
///   truncates it in web-core.
/// - The item is the swiper's element child number `index`, text nodes skipped ([`element_child`]),
///   web-core's `children.item(index)`. A negative index, or one past the last child, names no item
///   and is code 4 with nothing moved, which is Android's answer and the rule `selectTab` follows;
///   web-core does nothing and answers 0.
/// - `smooth` is read with JavaScript truthiness, as web-core's `smooth ? 'smooth' : 'instant'`
///   reads it, and a missing one is `true`, web-core's default. Android's default is the
///   `smooth-scroll` attribute (`mSmoothScroll`, `XSwiperUI.java:89`, itself `true` by default),
///   which only an autoplay turn reads here ([`turn`]).
/// - Android's `direction` (`"begin"` or `"end"`: which way a circular swiper turns,
///   `XSwiperUI.java:767-772`) is not read: web-core has no such param.
///
/// Where the item lands: on the main axis, the [`ScrollLogicalPosition`] of
/// the item's computed `scroll-snap-align` — `start` at the start, `center` at
/// the center, `end` at the end, and `none` at the start; on the cross axis,
/// `nearest`. css-scroll-snap-1 places a snap position with the same snapport
/// (`scroll-padding`) and `scroll-margin` terms the scroll-into-view position
/// uses (`crates/dom/src/scroll/snap.rs`, `crates/dom/src/scroll/into_view.rs`),
/// so the item comes to rest exactly on its snap position. web-core computes
/// an offset per mode instead (`XSwiper.ts:104-129`): the item's
/// `offsetLeft`/`offsetTop`, less a third of the item's size in
/// `flat-coverflow`, which centres that mode's 60% items; in `coverflow` and
/// `carry` it aims at the start and the items' mandatory
/// `scroll-snap-align: center` moves the swiper on to the centre. Both end on
/// the same position in every mode. The snap alignment avoids web-core's
/// second movement in `coverflow` and `carry`, and follows an author's own
/// `scroll-snap-align` on an item.
///
/// `container: nearest` is the item's nearest scroll container over the flat
/// tree, the shadow `#content` ([`content`]), the same box an item's
/// `scroll-initial-target` finds. Its `overflow` is a shadow-sheet declaration
/// no author rule reaches, so it is always a scroll container and, unlike
/// `selectTab`, the method needs no guard against scrolling an ancestor.
///
/// A smooth turn is animated by the painter and leaves the document's offset
/// where it was until the painter posts it back; an instant one moves the
/// document at once. A swiper or item with no box (`display: none`, or a
/// `wrapper` child, which is `display: contents`) has nothing to scroll, and
/// the call succeeds with nothing moved, as `selectTab` does. Positions are
/// the last completed layout's: a UI method never flushes.
pub(super) fn scroll_to(
    document: &mut LynxDocument,
    swiper: NodeId,
    params: &str,
) -> Result<(), InvalidParams> {
    let params: Value = serde_json::from_str(params).map_err(|_| InvalidParams)?;
    let index = params
        .get("index")
        .and_then(Value::as_f64)
        .ok_or(InvalidParams)?
        .trunc();
    let behavior = if params.get("smooth").is_none_or(is_truthy) {
        ScrollBehavior::Smooth
    } else {
        ScrollBehavior::Instant
    };
    let item = element_child(document, swiper, index).ok_or(InvalidParams)?;
    let vertical = is_vertical(document, swiper);
    let align = document.get(item).and_then(Node::computed_style).map_or(
        ScrollSnapAlignKeyword::None,
        |style| {
            let align = style.get_scroll_snap_align();
            if vertical {
                align.block()
            } else {
                align.inline()
            }
        },
    );
    let main = match align {
        ScrollSnapAlignKeyword::None | ScrollSnapAlignKeyword::Start => {
            ScrollLogicalPosition::Start
        }
        ScrollSnapAlignKeyword::Center => ScrollLogicalPosition::Center,
        ScrollSnapAlignKeyword::End => ScrollLogicalPosition::End,
    };
    let (block, inline) = if vertical {
        (main, ScrollLogicalPosition::Nearest)
    } else {
        (ScrollLogicalPosition::Nearest, main)
    };
    document.scroll_into_view(
        item,
        ScrollIntoViewOptions {
            behavior,
            block,
            inline,
            container: ScrollIntoViewContainer::Nearest,
        },
    );
    Ok(())
}

/// Whether `swiper` is a column: `vertical` present and not `"false"`, the
/// UA rules' `[vertical]:not([vertical="false"])`.
fn is_vertical(document: &LynxDocument, swiper: NodeId) -> bool {
    document
        .get(swiper)
        .is_some_and(|node| flag(node, "vertical"))
}

/// Whether `element` carries the boolean attribute `name`: present and not
/// `"false"`, the attribute web-core's false-filtering leaves.
fn flag(element: &Node<()>, name: &str) -> bool {
    element
        .attribute(name)
        .is_some_and(|value| value != "false")
}

/// One autoplay turn of `swiper`: to the item after the current one, or
/// from the last item to the first when it is `circular`
/// (`XSwiperAutoScroll.ts:22-33`).
///
/// The positions are the swiper's snap positions on its main axis
/// ([`dom::Document::snap_positions`]), one per item, in order. The current
/// item is the one whose position is nearest the offset, the first on a tie.
/// That is web-core's "item whose centre is nearest the scrollport's mid"
/// (`XSwiper.ts:37-91`, the mid at 40% of the scrollport in `carousel`)
/// restated: items are equal on the main axis, and each one's position is
/// its centre less the same distance — half the scrollport for a centred
/// item, half the item for a start-aligned one, which in `carousel` is the
/// 40% web-core uses. The target is the next item's snap position, which is
/// where web-core's `#scrollToIndex` ends after its mandatory snap
/// (`:104-129`); on the last item without `circular` nothing moves, and the
/// timer goes on.
///
/// The turn is smooth unless `smooth-scroll` is present with any value,
/// `"false"` included, which is web-core's reading
/// (`getAttribute('smooth-scroll') === null`, `XSwiper.ts:101`, with
/// `smooth-scroll` exempt from its false-filtering, `:28-31`). Android reads
/// the attribute as a boolean that defaults to `true`
/// (`XSwiperUI.java:877-885`), and turns over `duration` milliseconds, which
/// web-core never reads. A `circular` swiper's turn from the last item runs
/// along the shorter arc of its circle, the painter's rule for a smooth
/// request, where web-core re-slots its first item to travel forward and
/// travels backward with two items (`XSwiperCircular.ts:44-118`).
///
/// It is a request the painter declines while a pointer holds the swiper
/// ([`dom::Document::scroll_to_yielding`]), so the document's offset stays
/// where it is until the painter posts it back, for a `smooth-scroll` turn
/// too. A turn that comes before then counts from the old item and asks for
/// the same one again. A pending `scrollTo` a script made is not replaced.
///
/// The offset, the positions and the turn are `#content`'s ([`content`]),
/// the scroll container. Reads the last completed layout and never flushes.
/// A swiper with no box, no snap positions or no items moves nothing.
fn turn(document: &mut LynxDocument, swiper: NodeId) {
    let Some(element) = document.get(swiper) else {
        return;
    };
    let vertical = flag(element, "vertical");
    let circular = flag(element, "circular");
    let behavior = if element.attribute("smooth-scroll").is_some() {
        ScrollBehavior::Instant
    } else {
        ScrollBehavior::Smooth
    };
    let Some(content) = content(document, swiper) else {
        return;
    };
    let (Some(scroll_box), Some(positions)) = (
        document.scroll_box(content),
        document.snap_positions(content),
    ) else {
        return;
    };
    let offset = scroll_box.offset;
    let (axis, at) = if vertical {
        (positions.y(), offset.y)
    } else {
        (positions.x(), offset.x)
    };
    let Some(points) = axis.map(|axis| axis.points) else {
        return;
    };
    let distance = |index: usize| {
        let point = points[index];
        (point.min - at).max(at - point.max).max(0.0)
    };
    let Some(current) = (0..points.len()).min_by(|a, b| distance(*a).total_cmp(&distance(*b)))
    else {
        return;
    };
    let next = if current + 1 < points.len() {
        current + 1
    } else if circular {
        0
    } else {
        return;
    };
    let target = points[next].min;
    let to = if vertical {
        Vector2D::new(offset.x, target)
    } else {
        Vector2D::new(target, offset.y)
    };
    document.scroll_to_yielding(content, to, behavior);
}

/// Writes the number of elements assigned to `swiper`'s slot into its
/// `--swiper-count` hint: web-core's `childElementCount`, which counts
/// elements alone, so a text child — a slottable too — is not an item.
fn count_items(document: &mut LynxDocument, swiper: NodeId) {
    let count = slot(document, swiper).map_or(0, |slot| {
        document
            .assigned_nodes(slot)
            .iter()
            .filter(|&&node| document.get(node).is_some_and(Node::is_element))
            .count()
    });
    document.set_presentational_hint(swiper, COUNT_PROPERTY, &count.to_string());
}

/// The `<slot>` in the swiper's `#content`, its only child.
fn slot(document: &LynxDocument, swiper: NodeId) -> Option<NodeId> {
    let content = content(document, swiper)?;
    document
        .get(content)?
        .children()
        .find(|child| child.is_element())
        .map(Node::id)
}

/// The swiper's scroll container, `#content`, the first element of its
/// shadow tree.
pub(crate) fn content(document: &LynxDocument, swiper: NodeId) -> Option<NodeId> {
    let root = document.shadow_root(swiper)?;
    document
        .get(root)?
        .children()
        .find(|child| child.is_element())
        .map(Node::id)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::float_cmp)] // Explicit pixel sizes lay out exactly.

    use std::time::Duration;

    use dom::scroll::ScrollBehavior;
    use dom::stylo::properties::PropertyId;
    use dom::stylo::values::computed::{CSSPixelLength, Display, Overflow};
    use dom::{
        MethodCall, MethodError, MethodOutcome, NodeId, Point2D, Rect, Size2D, StylesheetOrigin,
        Vector2D,
    };

    use super::super::test_support::{
        child, display, document, element_under, overflow, style_of, with_autoplays,
    };
    use super::super::{InvalidParams, LynxDocument};
    use super::{Autoplays, SWIPER_ITEM_TAG, SWIPER_TAG, autoplay_period, scroll_to};
    use crate::clock::ClockInstant;

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

    fn rect(document: &LynxDocument, element: NodeId) -> (f32, f32, f32, f32) {
        let layout = document.rounded_layout(element).expect("a live element");
        (
            layout.location.x,
            layout.location.y,
            layout.size.width,
            layout.size.height,
        )
    }

    /// A swiper with `count` items under the page, `style` inline on it and
    /// `attributes` set before anything is laid out.
    fn build(
        document: &mut LynxDocument,
        style: &str,
        attributes: &[(&str, &str)],
        count: usize,
    ) -> (NodeId, Vec<NodeId>) {
        let swiper = child(document, SWIPER_TAG, style);
        for (name, value) in attributes {
            document.set_attribute(swiper, name, value);
        }
        let items = (0..count)
            .map(|_| element_under(document, swiper, SWIPER_ITEM_TAG, ""))
            .collect();
        (swiper, items)
    }

    /// The swiper's scroll container, `#content` in its shadow tree.
    fn scroller(document: &LynxDocument, swiper: NodeId) -> NodeId {
        super::content(document, swiper).expect("every swiper has a shadow `#content`")
    }

    /// The swiper's dot strip, `#indicator` in its shadow tree.
    fn indicator(document: &LynxDocument, swiper: NodeId) -> NodeId {
        let shadow = document
            .shadow_root(swiper)
            .expect("a swiper has a shadow tree");
        document
            .get(shadow)
            .expect("a live shadow root")
            .child_ids()[1]
    }

    /// The swiper's snap positions on its main axis, as single offsets.
    fn snap_points(document: &LynxDocument, swiper: NodeId, vertical: bool) -> Vec<f32> {
        let positions = document
            .snap_positions(scroller(document, swiper))
            .expect("a snapping swiper");
        let axis = if vertical {
            positions.y().expect("snaps on y")
        } else {
            positions.x().expect("snaps on x")
        };
        axis.points
            .iter()
            .map(|point| {
                assert_eq!(point.min, point.max, "an item is no larger than its port");
                point.min
            })
            .collect()
    }

    // --- the UA rules -------------------------------------------------------

    /// The host keeps the container defaults; its shadow `#content` is the
    /// scroll container, a row flex box that fills it.
    #[test]
    fn a_swiper_scrolls_x_snapping_mandatory_in_a_row() {
        let mut document = document();
        let (swiper, _) = build(&mut document, "width: 200px; height: 100px", &[], 0);
        document.layout();
        for (property, expected) in [
            ("box-sizing", "border-box"),
            ("position", "relative"),
            ("contain", "none"),
            ("scroll-snap-type", "none"),
            ("timeline-scope", "--swiper-scroller"),
            ("--swiper-current", "-1"),
            ("--swiper-count", "0"),
        ] {
            assert_eq!(value(&document, swiper, property), expected, "{property}");
        }
        assert_eq!(
            overflow(&document, swiper),
            (Overflow::Clip, Overflow::Clip)
        );
        assert_eq!(
            style_of(&document, swiper).slow_clone_display(),
            Display::Linear
        );

        let content = scroller(&document, swiper);
        assert_eq!(
            overflow(&document, content),
            (Overflow::Scroll, Overflow::Hidden),
            "the clipped y computes to `hidden` beside a scrolling x"
        );
        for (property, expected) in [
            ("width", "100%"),
            ("height", "100%"),
            ("contain", "content"),
            ("justify-content", "flex-start"),
            ("scroll-snap-type", "x mandatory"),
            ("flex-direction", "row"),
            ("flex-wrap", "nowrap"),
            ("overscroll-behavior-x", "auto"),
            ("overscroll-behavior-y", "auto"),
            ("scroll-timeline-name", "--swiper-scroller"),
            ("scroll-timeline-axis", "inline"),
        ] {
            assert_eq!(value(&document, content, property), expected, "{property}");
        }
        assert_eq!(
            style_of(&document, content).slow_clone_display(),
            Display::Flex
        );
        assert_eq!(rect(&document, content), (0.0, 0.0, 200.0, 100.0));
    }

    #[test]
    fn a_vertical_swiper_scrolls_y_in_a_column_unless_it_says_false() {
        for (attribute, vertical) in [("", true), ("true", true), ("false", false)] {
            let mut document = document();
            let (swiper, _) = build(&mut document, "", &[("vertical", attribute)], 0);
            document.layout();
            let (overflows, snap, direction, axis) = if vertical {
                (
                    (Overflow::Hidden, Overflow::Scroll),
                    "y mandatory",
                    "column",
                    "block",
                )
            } else {
                (
                    (Overflow::Scroll, Overflow::Hidden),
                    "x mandatory",
                    "row",
                    "inline",
                )
            };
            let content = scroller(&document, swiper);
            assert_eq!(overflow(&document, content), overflows, "{attribute:?}");
            assert_eq!(value(&document, content, "scroll-snap-type"), snap);
            assert_eq!(value(&document, content, "flex-direction"), direction);
            assert_eq!(value(&document, content, "scroll-timeline-axis"), axis);
        }
    }

    /// `vertical` set after the first layout turns `#content` too: a `:host()`
    /// rule in the shadow sheet follows the host's attribute.
    #[test]
    fn a_later_vertical_turns_the_shadow_scroller() {
        let (mut document, swiper, items) = mode_swiper("normal", false, 2);
        document.layout();
        document.set_attribute(swiper, "vertical", "");
        document.set_inline_style(swiper, "width: 100px; height: 200px");
        document.layout();
        let content = scroller(&document, swiper);
        assert_eq!(value(&document, content, "flex-direction"), "column");
        assert_eq!(rect(&document, items[1]), (0.0, 200.0, 100.0, 200.0));
    }

    #[test]
    fn an_item_fills_the_main_axis_and_snaps_at_its_start() {
        let mut document = document();
        let (_, items) = build(&mut document, "", &[], 1);
        document.layout();
        for (property, expected) in [
            ("width", "100%"),
            ("height", "100%"),
            ("flex-grow", "0"),
            ("flex-shrink", "0"),
            ("flex-basis", "auto"),
            ("scroll-snap-align", "start"),
            ("scroll-initial-target", "none"),
            ("animation-name", "none"),
            ("position", "relative"),
            ("box-sizing", "border-box"),
        ] {
            assert_eq!(value(&document, items[0], property), expected, "{property}");
        }
    }

    /// An author main axis on the swiper, and an author size on an item,
    /// `!important` or not, still give a row of full-width items.
    #[test]
    fn author_styles_cannot_move_the_main_axis_or_resize_an_item() {
        let mut document = document();
        document.add_stylesheet(
            ".swiper { display: flex; flex-direction: column !important; flex-wrap: wrap !important; }
             .item { width: 50px !important; }",
            StylesheetOrigin::Author,
        );
        let (swiper, items) = build(&mut document, "width: 200px; height: 100px", &[], 3);
        document.add_class(swiper, "swiper");
        for item in &items {
            document.add_class(*item, "item");
        }
        document.layout();
        let placed: Vec<_> = items.iter().map(|item| rect(&document, *item)).collect();
        assert_eq!(
            placed,
            [
                (0.0, 0.0, 200.0, 100.0),
                (200.0, 0.0, 200.0, 100.0),
                (400.0, 0.0, 200.0, 100.0),
            ]
        );
    }

    /// A child that is no item generates no box, except a `wrapper`, which
    /// keeps generating none of its own.
    #[test]
    fn a_child_that_is_no_item_generates_no_box() {
        let mut document = document();
        let (swiper, items) = build(&mut document, "width: 200px; height: 100px", &[], 1);
        let view = element_under(&mut document, swiper, "view", "width: 10px; height: 10px");
        let wrapper = element_under(&mut document, swiper, "wrapper", "");
        document.layout();
        assert_eq!(
            style_of(&document, view).slow_clone_display(),
            Display::None
        );
        assert_eq!(
            style_of(&document, wrapper).slow_clone_display(),
            Display::Contents
        );
        assert_eq!(rect(&document, items[0]), (0.0, 0.0, 200.0, 100.0));
        assert_eq!(snap_points(&document, swiper, false), [0.0]);
    }

    /// An `x-swiper-item` outside a swiper is an ordinary container.
    #[test]
    fn an_item_outside_a_swiper_is_an_ordinary_container() {
        let mut document = document();
        let item = child(&mut document, SWIPER_ITEM_TAG, "");
        document.layout();
        assert_eq!(
            style_of(&document, item).slow_clone_display(),
            Display::Linear
        );
        assert_eq!(value(&document, item, "width"), "auto");
        assert_eq!(value(&document, item, "scroll-snap-align"), "none");
    }

    #[test]
    fn bounces_stretches_the_main_axis_unless_it_says_false() {
        for (attribute, bounces) in [
            (None, false),
            (Some(""), true),
            (Some("true"), true),
            (Some("false"), false),
        ] {
            for vertical in [false, true] {
                let mut document = document();
                let (swiper, _) = build(&mut document, "", &[], 0);
                if vertical {
                    document.set_attribute(swiper, "vertical", "");
                }
                if let Some(attribute) = attribute {
                    document.set_attribute(swiper, "bounces", attribute);
                }
                document.layout();
                let main = if bounces { "contain-bounce" } else { "auto" };
                let (x, y) = if vertical {
                    ("auto", main)
                } else {
                    (main, "auto")
                };
                assert_eq!(
                    value(
                        &document,
                        scroller(&document, swiper),
                        "overscroll-behavior-x"
                    ),
                    x,
                    "bounces={attribute:?} vertical={vertical}"
                );
                assert_eq!(
                    value(
                        &document,
                        scroller(&document, swiper),
                        "overscroll-behavior-y"
                    ),
                    y,
                    "bounces={attribute:?} vertical={vertical}"
                );
            }
        }
    }

    /// `circular` wraps `#content`'s main axis, present and not `"false"`,
    /// and wins over `bounces` on that axis when both are set: a circular
    /// axis has no edge to bounce. The cross axis stays `auto`.
    #[test]
    fn circular_wraps_the_main_axis_unless_it_says_false() {
        for (circular, bounces, main) in [
            (None, false, "auto"),
            (Some(""), false, "circular"),
            (Some("true"), false, "circular"),
            (Some("false"), false, "auto"),
            (Some(""), true, "circular"),
            (Some("false"), true, "contain-bounce"),
        ] {
            for vertical in [false, true] {
                let mut document = document();
                let (swiper, _) = build(&mut document, "", &[], 0);
                if vertical {
                    document.set_attribute(swiper, "vertical", "");
                }
                if bounces {
                    document.set_attribute(swiper, "bounces", "");
                }
                if let Some(circular) = circular {
                    document.set_attribute(swiper, "circular", circular);
                }
                document.layout();
                let (x, y) = if vertical {
                    ("auto", main)
                } else {
                    (main, "auto")
                };
                let content = scroller(&document, swiper);
                let case = format!("circular={circular:?} bounces={bounces} vertical={vertical}");
                assert_eq!(
                    value(&document, content, "overscroll-behavior-x"),
                    x,
                    "{case}"
                );
                assert_eq!(
                    value(&document, content, "overscroll-behavior-y"),
                    y,
                    "{case}"
                );
            }
        }
    }

    // --- the modes ----------------------------------------------------------

    /// One mode in one orientation: each item's rect, and the swiper's snap
    /// positions on its main axis.
    struct Geometry {
        mode: &'static str,
        vertical: bool,
        rects: [(f32, f32, f32, f32); 3],
        snaps: [f32; 3],
    }

    /// A 200px x 100px swiper horizontally, 100px x 200px vertically, of
    /// three items. Two things the numbers show:
    ///
    /// - The last item's end margin extends the scroll range, as a flex item's margin area does in
    ///   a scroll container's scrollable overflow (css-overflow-3 §3.3): horizontally the last
    ///   `carousel` item reaches its start (320) and the last `flat-coverflow` or `coverflow` item
    ///   its centre (240).
    /// - Every percentage margin resolves against the swiper's width, as CSS resolves a vertical
    ///   margin too, so under `vertical` the 20% is 20px, not 40px (web-core alike): the first
    ///   `flat-coverflow` item cannot centre (0, not -20), and the last item of every mode with a
    ///   trailing margin stops 20px short — `carousel` at 300, not 320, `flat-coverflow` and
    ///   `coverflow` at 200, not 220.
    const GEOMETRY: [Geometry; 10] = [
        Geometry {
            mode: "normal",
            vertical: false,
            rects: [
                (0.0, 0.0, 200.0, 100.0),
                (200.0, 0.0, 200.0, 100.0),
                (400.0, 0.0, 200.0, 100.0),
            ],
            snaps: [0.0, 200.0, 400.0],
        },
        Geometry {
            mode: "normal",
            vertical: true,
            rects: [
                (0.0, 0.0, 100.0, 200.0),
                (0.0, 200.0, 100.0, 200.0),
                (0.0, 400.0, 100.0, 200.0),
            ],
            snaps: [0.0, 200.0, 400.0],
        },
        Geometry {
            mode: "carousel",
            vertical: false,
            rects: [
                (0.0, 0.0, 160.0, 100.0),
                (160.0, 0.0, 160.0, 100.0),
                (320.0, 0.0, 160.0, 100.0),
            ],
            snaps: [0.0, 160.0, 320.0],
        },
        Geometry {
            mode: "carousel",
            vertical: true,
            rects: [
                (0.0, 0.0, 100.0, 160.0),
                (0.0, 160.0, 100.0, 160.0),
                (0.0, 320.0, 100.0, 160.0),
            ],
            snaps: [0.0, 160.0, 300.0],
        },
        Geometry {
            mode: "flat-coverflow",
            vertical: false,
            rects: [
                (40.0, 0.0, 120.0, 100.0),
                (160.0, 0.0, 120.0, 100.0),
                (280.0, 0.0, 120.0, 100.0),
            ],
            snaps: [0.0, 120.0, 240.0],
        },
        Geometry {
            mode: "flat-coverflow",
            vertical: true,
            rects: [
                (0.0, 20.0, 100.0, 120.0),
                (0.0, 140.0, 100.0, 120.0),
                (0.0, 260.0, 100.0, 120.0),
            ],
            snaps: [0.0, 100.0, 200.0],
        },
        Geometry {
            mode: "coverflow",
            vertical: false,
            rects: [
                (40.0, 0.0, 120.0, 100.0),
                (160.0, 0.0, 120.0, 100.0),
                (280.0, 0.0, 120.0, 100.0),
            ],
            snaps: [0.0, 120.0, 240.0],
        },
        Geometry {
            mode: "coverflow",
            vertical: true,
            rects: [
                (0.0, 20.0, 100.0, 120.0),
                (0.0, 140.0, 100.0, 120.0),
                (0.0, 260.0, 100.0, 120.0),
            ],
            snaps: [0.0, 100.0, 200.0],
        },
        Geometry {
            mode: "carry",
            vertical: false,
            rects: [
                (0.0, 0.0, 200.0, 100.0),
                (200.0, 0.0, 200.0, 100.0),
                (400.0, 0.0, 200.0, 100.0),
            ],
            snaps: [0.0, 200.0, 400.0],
        },
        Geometry {
            mode: "carry",
            vertical: true,
            rects: [
                (0.0, 0.0, 100.0, 200.0),
                (0.0, 200.0, 100.0, 200.0),
                (0.0, 400.0, 100.0, 200.0),
            ],
            snaps: [0.0, 200.0, 400.0],
        },
    ];

    fn mode_swiper(
        mode: &str,
        vertical: bool,
        count: usize,
    ) -> (LynxDocument, NodeId, Vec<NodeId>) {
        let mut document = document();
        let style = if vertical {
            "width: 100px; height: 200px"
        } else {
            "width: 200px; height: 100px"
        };
        let mut attributes = vec![("mode", mode)];
        if vertical {
            attributes.push(("vertical", ""));
        }
        let (swiper, items) = build(&mut document, style, &attributes, count);
        (document, swiper, items)
    }

    #[test]
    fn each_mode_lays_its_items_out_on_the_main_axis() {
        for case in &GEOMETRY {
            let (mut document, swiper, items) = mode_swiper(case.mode, case.vertical, 3);
            document.layout();
            let placed: Vec<_> = items.iter().map(|item| rect(&document, *item)).collect();
            assert_eq!(
                placed, case.rects,
                "{} vertical={}",
                case.mode, case.vertical
            );
            assert_eq!(
                snap_points(&document, swiper, case.vertical),
                case.snaps,
                "{} vertical={}",
                case.mode,
                case.vertical
            );
            let centred = matches!(case.mode, "flat-coverflow" | "coverflow" | "carry");
            assert_eq!(
                value(&document, items[0], "scroll-snap-align"),
                if centred { "center" } else { "start" },
                "{}",
                case.mode
            );
        }
    }

    /// An unknown mode is `normal`.
    #[test]
    fn an_unknown_mode_is_normal() {
        let (mut document, swiper, items) = mode_swiper("cube", false, 2);
        document.layout();
        assert_eq!(rect(&document, items[1]), (200.0, 0.0, 200.0, 100.0));
        assert_eq!(snap_points(&document, swiper, false), [0.0, 200.0]);
    }

    /// Under `circular` the mode margins are off, as in web-core, whose
    /// margin rules all carry `:not([circular])`: the items pack edge to
    /// edge and the scroll range — the period the painter wraps on — is
    /// their extent alone, so the seam joins the last item to the first
    /// instead of showing the swiper's own background. `circular="false"`
    /// keeps the margins.
    #[test]
    fn circular_packs_the_items_without_the_mode_margins() {
        // The main-axis item size, and the maximum offset packed and with
        // the margins, of a 200px x 100px (100px x 200px) swiper of three.
        for (mode, vertical, item, packed_max, margined_max) in [
            ("carousel", false, 160.0, 280.0, 320.0),
            ("carousel", true, 160.0, 280.0, 300.0),
            ("flat-coverflow", false, 120.0, 160.0, 240.0),
            ("flat-coverflow", true, 120.0, 160.0, 200.0),
            ("coverflow", false, 120.0, 160.0, 240.0),
            ("coverflow", true, 120.0, 160.0, 200.0),
        ] {
            for (circular, packed) in [("", true), ("true", true), ("false", false)] {
                let (mut document, swiper, items) = mode_swiper(mode, vertical, 3);
                document.set_attribute(swiper, "circular", circular);
                document.layout();
                let case = format!("{mode} vertical={vertical} circular={circular:?}");
                let placed: Vec<_> = items.iter().map(|item| rect(&document, *item)).collect();
                let expected: Vec<_> = if packed {
                    [0.0, 1.0, 2.0]
                        .map(|k: f32| {
                            let at = k * item;
                            if vertical {
                                (0.0, at, 100.0, item)
                            } else {
                                (at, 0.0, item, 100.0)
                            }
                        })
                        .to_vec()
                } else {
                    GEOMETRY
                        .iter()
                        .find(|case| case.mode == mode && case.vertical == vertical)
                        .expect("a listed mode")
                        .rects
                        .to_vec()
                };
                assert_eq!(placed, expected, "{case}");
                let max = document
                    .scroll_box(scroller(&document, swiper))
                    .expect("a scroll container")
                    .max_offset();
                let along = if packed { packed_max } else { margined_max };
                let expected_max = if vertical {
                    Vector2D::new(0.0, along)
                } else {
                    Vector2D::new(along, 0.0)
                };
                assert_eq!(max, expected_max, "{case}");
            }
        }
    }

    /// `coverflow` and `carry` export their transform to the painter as a
    /// curve that reads the swiper's scroll slot: nothing ticks on the main
    /// thread and no curve reads the clock. `coverflow`'s turn is a
    /// translation, a skew and a scale, which a 2D affine follows. `normal`,
    /// `carousel` and `flat-coverflow` animate nothing.
    #[test]
    fn the_animated_modes_export_a_scroll_driven_transform() {
        for vertical in [false, true] {
            for (mode, animated) in [
                ("normal", false),
                ("carousel", false),
                ("flat-coverflow", false),
                ("coverflow", true),
                ("carry", true),
            ] {
                let (mut document, _, items) = mode_swiper(mode, vertical, 3);
                let frame = document.commit();
                let mut exported: Vec<NodeId> = frame
                    .animation_slots()
                    .iter()
                    .map(|slot| slot.node)
                    .collect();
                exported.sort_unstable_by_key(|node| node.to_bits());
                let expected = if animated { items.clone() } else { Vec::new() };
                assert_eq!(exported, expected, "{mode} vertical={vertical}");
                assert!(
                    !frame.has_live_curves() && !frame.needs_main_ticks(),
                    "{mode} vertical={vertical}: nothing ticks"
                );
                assert!(!frame.animations_active(), "{mode} vertical={vertical}");
            }
        }
    }

    /// `carry` at rest on the first item: the item in the scrollport is at
    /// the middle of its view timeline, unscaled, and the next one is at its
    /// start, at `0%`'s `scale(0.6)`. Scrolling one item on swaps them.
    #[test]
    fn carry_scales_the_items_beside_the_current_one() {
        for vertical in [false, true] {
            let (mut document, swiper, items) = mode_swiper("carry", vertical, 3);
            document.commit();
            assert_eq!(value(&document, items[0], "transform"), "scale(1)");
            assert_eq!(value(&document, items[1], "transform"), "scale(0.6)");
            let offset = if vertical {
                Vector2D::new(0.0, 200.0)
            } else {
                Vector2D::new(200.0, 0.0)
            };
            // An exported curve's cascade value is the commit's: a commit
            // that repaints — here because the scroll moved far through its
            // encode window, as the runtime notes it — re-samples it.
            document.scroll_to(scroller(&document, swiper), offset);
            document.note_scroll_windows_stale();
            document.commit();
            assert_eq!(value(&document, items[1], "transform"), "scale(1)");
            assert_eq!(value(&document, items[0], "transform"), "scale(0.6)");
        }
    }

    /// What `item`'s computed transform does to its border box, read along
    /// the swiper's main axis.
    struct Turn {
        /// The scale along the main axis.
        along: f32,
        /// The scale along the cross axis.
        across: f32,
        /// The cross-axis movement per unit along the main axis.
        skew: f32,
        /// Where the transform puts the item's two main-axis edges, as
        /// offsets from its centre: the start one, then the end one.
        edges: [f32; 2],
    }

    fn turn(document: &LynxDocument, item: NodeId, vertical: bool) -> Turn {
        let (.., width, height) = rect(document, item);
        let reference = Rect::new(
            Point2D::origin(),
            Size2D::new(CSSPixelLength::new(width), CSSPixelLength::new(height)),
        );
        let (matrix, _) = style_of(document, item)
            .get_box()
            .transform
            .to_transform_3d_matrix(Some(&reference))
            .expect("a transform list with a reference box converts");
        let (along, across, skew, shift, half) = if vertical {
            (matrix.m22, matrix.m11, matrix.m21, matrix.m42, height / 2.0)
        } else {
            (matrix.m11, matrix.m22, matrix.m12, matrix.m41, width / 2.0)
        };
        Turn {
            along,
            across,
            skew,
            edges: [shift - half * along, shift + half * along],
        }
    }

    /// `coverflow` at rest on the middle one of three 120px items, a row and
    /// a column: the current item is untransformed, and the items beside it,
    /// at 87.5% and 12.5% of their view timelines, stand 72% of the way to
    /// the full pose. Each shows `0.649` of its main-axis size and `0.856` of
    /// its cross-axis one, skewed by `0.649 × tan 10.1°` with the sign of its
    /// side, and the edge it turns to the current item — its end edge before
    /// the current item, its start edge after — is where layout put it, 60px
    /// from its centre, so it takes nothing from its neighbour.
    #[test]
    fn coverflow_turns_the_items_beside_the_current_one() {
        let close = |actual: f32, expected: f32| (actual - expected).abs() < 1e-3;
        for vertical in [false, true] {
            let (mut document, swiper, items) = mode_swiper("coverflow", vertical, 3);
            document.commit();
            let along = snap_points(&document, swiper, vertical)[1];
            let offset = if vertical {
                Vector2D::new(0.0, along)
            } else {
                Vector2D::new(along, 0.0)
            };
            document.scroll_to(scroller(&document, swiper), offset);
            document.note_scroll_windows_stale();
            document.commit();

            let current = turn(&document, items[1], vertical);
            assert!(
                close(current.along, 1.0)
                    && close(current.across, 1.0)
                    && close(current.skew, 0.0)
                    && close(current.edges[0], -60.0)
                    && close(current.edges[1], 60.0),
                "vertical={vertical}: the current item is flat"
            );
            for (item, side, inner) in [(items[0], -1.0, 1), (items[2], 1.0, 0)] {
                let beside = turn(&document, item, vertical);
                assert!(
                    close(beside.along, 0.649) && close(beside.across, 0.8556),
                    "vertical={vertical} side={side}: {} by {}",
                    beside.along,
                    beside.across
                );
                assert!(
                    close(beside.skew, side * 0.1157),
                    "vertical={vertical} side={side}: skew {}",
                    beside.skew
                );
                assert!(
                    close(beside.edges[inner], -side * 60.0),
                    "vertical={vertical} side={side}: the inner edge is at {}",
                    beside.edges[inner]
                );
            }
        }
    }

    // --- current ------------------------------------------------------------

    /// `current` names the item the first commit shows, and the position
    /// the painter's at-rest snap rule settles that scroll on is the item's
    /// snap position: its start in `normal` and `carousel`, its centre in
    /// the other three. The scroll container an item's
    /// `scroll-initial-target: nearest` finds over the flat tree is the
    /// shadow `#content`, and once settled the active dot is that item's.
    #[test]
    fn current_starts_on_the_items_snap_position_in_every_mode() {
        for case in &GEOMETRY {
            for (current, index) in [("1", 1), ("2", 2)] {
                let (mut document, swiper, items) = mode_swiper(case.mode, case.vertical, 3);
                document.set_attribute(swiper, "current", current);
                document.commit();
                let targets: Vec<_> = items
                    .iter()
                    .map(|item| value(&document, *item, "scroll-initial-target"))
                    .collect();
                let mut expected = ["none"; 3];
                expected[index] = "nearest";
                assert_eq!(targets, expected, "{} current={current}", case.mode);
                let at = document.scroll_offset(scroller(&document, swiper));
                let settled = document.settle_scroll(scroller(&document, swiper), at);
                let expected = if case.vertical {
                    Vector2D::new(0.0, case.snaps[index])
                } else {
                    Vector2D::new(case.snaps[index], 0.0)
                };
                assert_eq!(
                    settled, expected,
                    "{} vertical={} current={current}",
                    case.mode, case.vertical
                );
                // The settled offset reaches the main thread as an adopted
                // scroll post does, and the strip re-samples.
                let content = scroller(&document, swiper);
                document.advance_scroll_timelines(&[content]);
                document.commit();
                #[expect(clippy::cast_precision_loss, reason = "index < 3")]
                let dot = index as f32 * 13.44;
                assert!(
                    (active_offset(&document, swiper, case.vertical) - dot).abs() < 0.01,
                    "the settled active dot: {} vertical={} current={current}",
                    case.mode,
                    case.vertical
                );
            }
        }
    }

    /// Under `circular` the items pack without the mode margins, so an end
    /// item's snap position lies past `max_offset` or before `0`, and the
    /// document admits it on the circle — a period of `max_offset` plus the
    /// scrollport — instead of clamping it. `carousel` (items 160, `max`
    /// 280, period 480): the last item's start is 320. `flat-coverflow`
    /// (items 120, `max` 160, period 360): the first item's centre is −40,
    /// so 320 on the circle, and the last item's is 200. `current` settles
    /// there and `scrollTo` lands there, in both orientations, and the
    /// active dot is the item's: the scroll timeline reads 320 in
    /// `flat-coverflow` at its representative −40, the first dot.
    #[test]
    fn current_and_scroll_to_land_across_the_seam_under_circular() {
        for (mode, snaps, cases) in [
            (
                "carousel",
                [0.0, 160.0, 320.0],
                [(1_usize, 160.0), (2, 320.0)],
            ),
            (
                "flat-coverflow",
                [80.0, 200.0, 320.0],
                [(0_usize, 320.0), (2, 200.0)],
            ),
        ] {
            for vertical in [false, true] {
                for (index, position) in cases {
                    let label = format!("{mode} vertical={vertical} index={index}");
                    let (mut document, swiper, _) = mode_swiper(mode, vertical, 3);
                    document.set_attribute(swiper, "circular", "");
                    document.set_attribute(swiper, "current", &index.to_string());
                    document.commit();
                    let content = scroller(&document, swiper);
                    assert_eq!(snap_points(&document, swiper, vertical), snaps, "{label}");
                    let at = document.scroll_offset(content);
                    assert_eq!(
                        document.settle_scroll(content, at),
                        main_axis(vertical, position),
                        "current settles on the item: {label}"
                    );
                    document.advance_scroll_timelines(&[content]);
                    document.commit();
                    #[expect(clippy::cast_precision_loss, reason = "index < 3")]
                    let dot = index as f32 * 13.44;
                    assert!(
                        (active_offset(&document, swiper, vertical) - dot).abs() < 0.01,
                        "the active dot: {label}"
                    );

                    let (mut document, swiper, _) = mode_swiper(mode, vertical, 3);
                    document.set_attribute(swiper, "circular", "");
                    document.layout();
                    let params = format!(r#"{{"index":{index},"smooth":false}}"#);
                    assert_eq!(scroll_to(&mut document, swiper, &params), Ok(()), "{label}");
                    assert_eq!(
                        document.scroll_offset(scroller(&document, swiper)),
                        main_axis(vertical, position),
                        "scrollTo lands on the item: {label}"
                    );
                }
            }
        }
    }

    #[test]
    fn a_current_naming_no_item_starts_on_the_first() {
        for current in ["-1", "3", "abc", "1.5", ""] {
            let (mut document, swiper, items) = mode_swiper("normal", false, 3);
            document.set_attribute(swiper, "current", current);
            document.commit();
            for item in &items {
                assert_eq!(value(&document, *item, "scroll-initial-target"), "none");
            }
            assert_eq!(
                document.scroll_offset(scroller(&document, swiper)),
                Vector2D::zero(),
                "{current:?}"
            );
        }
    }

    /// A later `current` turns the swiper, instantly.
    #[test]
    fn changing_current_turns_the_swiper_instantly() {
        let (mut document, swiper, _) = mode_swiper("normal", false, 3);
        document.set_attribute(swiper, "current", "1");
        document.commit();
        assert_eq!(
            document.scroll_offset(scroller(&document, swiper)),
            Vector2D::new(200.0, 0.0)
        );
        document.set_attribute(swiper, "current", "2");
        assert_eq!(
            pending(&mut document, swiper),
            Some((Vector2D::new(400.0, 0.0), ScrollBehavior::Instant))
        );
    }

    /// The scroll request the next commit carries for `swiper`.
    fn pending(
        document: &mut LynxDocument,
        swiper: NodeId,
    ) -> Option<(Vector2D<f32>, ScrollBehavior)> {
        let content = scroller(document, swiper);
        let frame = document.commit();
        let index = frame
            .slot_of(content)
            .expect("the swiper's scroller has a slot");
        frame.scroll_slots()[index as usize]
            .request
            .map(|request| (request.target, request.behavior))
    }

    // --- the shadow tree ----------------------------------------------------

    /// `#content` holding a default slot, then `#indicator`; the items are
    /// the host's children, assigned to the slot, and the shadow tree is out
    /// of every node-tree path the realm reads: `childElementIds` (a node's
    /// children), `parentNode`, and the selector queries.
    #[test]
    fn the_shadow_tree_is_content_with_a_slot_then_the_indicator() {
        let mut document = document();
        let (swiper, items) = build(&mut document, "width: 200px; height: 100px", &[], 2);
        document.layout();
        let shadow = document.shadow_root(swiper).expect("constructed with one");
        let parts = document
            .get(shadow)
            .expect("a live shadow root")
            .child_ids()
            .to_vec();
        assert_eq!(parts.len(), 2);
        let [content, indicator] = [parts[0], parts[1]];
        assert_eq!(content, scroller(&document, swiper));
        for (part, id) in [(content, "content"), (indicator, "indicator")] {
            let node = document.get(part).expect("a live part");
            assert_eq!(node.tag_name(), Some("div"));
            assert_eq!(node.attribute("id"), Some(id));
        }
        let slot = document.get(content).expect("live").child_ids()[0];
        assert_eq!(document.get(slot).expect("live").tag_name(), Some("slot"));
        assert_eq!(document.assigned_nodes(slot), items.as_slice());
        assert_eq!(display(&document, slot), Display::Contents);

        assert_eq!(document.get(swiper).expect("live").child_ids(), items);
        for item in &items {
            assert_eq!(document.get(*item).expect("live").parent_id(), Some(swiper));
        }
        let page = document.document_element().id();
        for selector in ["div", "slot", "#content", "#indicator", "x-swiper > *"] {
            let found = document
                .query_selector_all(page, selector)
                .expect("a valid selector");
            let expected = if selector == "x-swiper > *" {
                items.clone()
            } else {
                Vec::new()
            };
            assert_eq!(found, expected, "{selector}");
        }
        assert_eq!(
            document.scroll_box(swiper),
            None,
            "the host does not scroll"
        );
        assert!(document.scroll_box(content).is_some());
    }

    /// `--swiper-count` is the elements assigned to the slot — every element
    /// child: a child that is no item counts, a text node, assigned too, does
    /// not. Kept on `slotchange`: appends, a removal, a freeing removal.
    #[test]
    fn the_count_follows_the_element_children() {
        let mut document = document();
        let (swiper, items) = build(&mut document, "", &[], 0);
        document.layout();
        assert_eq!(value(&document, swiper, "--swiper-count"), "0");
        let items: Vec<_> = items
            .into_iter()
            .chain((0..4).map(|_| element_under(&mut document, swiper, SWIPER_ITEM_TAG, "")))
            .collect();
        element_under(&mut document, swiper, "view", "");
        let text = document.create_text_node("x", ());
        document.append_child(swiper, text);
        document.layout();
        assert_eq!(value(&document, swiper, "--swiper-count"), "5");
        let indicator = indicator(&document, swiper);
        assert_eq!(
            value(&document, indicator, "--swiper-count"),
            "5",
            "inherited"
        );
        document.remove_element(items[0]);
        document.drop_element(items[1]);
        document.layout();
        assert_eq!(value(&document, swiper, "--swiper-count"), "3");
    }

    // --- the indicator ------------------------------------------------------

    /// The strip is `count` dot pitches long, 0.6rem across, centred on the
    /// main axis 0.5rem in from the host's bottom (horizontal) or right
    /// (vertical) edge. `1rem` is 16px here, so a dot is 9.6px and a pitch
    /// 13.44px. Its layout box is before the centring transform.
    #[test]
    fn the_strip_is_one_pitch_per_item_centred_on_the_main_axis() {
        for (vertical, count, length, rect_expected) in [
            (false, 0, "0px", (100.0, 82.0, 0.0, 10.0)),
            (false, 1, "13.44px", (100.0, 82.0, 13.0, 10.0)),
            (false, 5, "67.2px", (100.0, 82.0, 67.0, 10.0)),
            (true, 0, "0px", (182.0, 100.0, 10.0, 0.0)),
            (true, 1, "13.44px", (182.0, 100.0, 10.0, 13.0)),
            (true, 5, "67.2px", (182.0, 100.0, 10.0, 67.0)),
        ] {
            let (mut document, swiper, _) = mode_swiper("normal", vertical, count);
            if vertical {
                document.set_inline_style(swiper, "width: 200px; height: 200px");
            }
            document.layout();
            let indicator = indicator(&document, swiper);
            let case = format!("vertical={vertical} count={count}");
            assert_eq!(display(&document, indicator), Display::Flex, "{case}");
            for (property, expected) in [
                ("position", "absolute"),
                ("z-index", "100"),
                ("pointer-events", "auto"),
            ] {
                assert_eq!(value(&document, indicator, property), expected, "{case}");
            }
            let (main, cross, repeat, transform) = if vertical {
                ("height", "width", "no-repeat, repeat-y", "translateY(-50%)")
            } else {
                ("width", "height", "no-repeat, repeat-x", "translateX(-50%)")
            };
            assert_eq!(value(&document, indicator, main), length, "{case}");
            assert_eq!(value(&document, indicator, cross), "9.6px", "{case}");
            assert_eq!(
                value(&document, indicator, "background-repeat"),
                repeat,
                "{case}"
            );
            assert_eq!(
                value(&document, indicator, "transform"),
                transform,
                "{case}"
            );
            assert_eq!(rect(&document, indicator), rect_expected, "{case}");
        }
    }

    /// web-core's colours by default — inactive `#ffffff4d`, active `white`
    /// — and the attributes' when they are colours.
    #[test]
    fn the_dot_colours_come_from_the_attributes() {
        for (attributes, active, inactive) in [
            (vec![], "rgb(255, 255, 255)", "rgba(255, 255, 255, 0.3)"),
            (
                vec![
                    ("indicator-color", "red"),
                    ("indicator-active-color", "#0000ff"),
                ],
                "rgb(0, 0, 255)",
                "rgb(255, 0, 0)",
            ),
            (
                vec![
                    ("indicator-color", "nonsense"),
                    ("indicator-active-color", ""),
                ],
                "rgb(255, 255, 255)",
                "rgba(255, 255, 255, 0.3)",
            ),
        ] {
            let mut document = document();
            let (swiper, _) = build(&mut document, "width: 200px; height: 100px", &attributes, 3);
            document.layout();
            let image = value(&document, indicator(&document, swiper), "background-image");
            assert_eq!(
                image,
                format!(
                    "radial-gradient(circle closest-side, {active} 0%, {active} 95%, \
                     rgba(0, 0, 0, 0) 105%), radial-gradient(circle closest-side, {inactive} 0%, \
                     {inactive} 95%, rgba(0, 0, 0, 0) 105%)"
                ),
                "{attributes:?}"
            );
        }
    }

    /// Shown by default and by `"true"`; hidden by any other present value,
    /// web-core's literal `"{{false}}"` included.
    #[test]
    fn indicator_dots_hides_the_strip_unless_absent_or_true() {
        for (attribute, shown) in [
            (None, true),
            (Some("true"), true),
            (Some(""), false),
            (Some("false"), false),
            (Some("{{false}}"), false),
        ] {
            let mut document = document();
            let (swiper, _) = build(&mut document, "width: 200px; height: 100px", &[], 3);
            if let Some(attribute) = attribute {
                document.set_attribute(swiper, "indicator-dots", attribute);
            }
            document.layout();
            let expected = if shown { Display::Flex } else { Display::None };
            assert_eq!(
                display(&document, indicator(&document, swiper)),
                expected,
                "{attribute:?}"
            );
        }
    }

    /// The active dot's offset along the strip, from the first of
    /// `background-position-x` (`-y` when vertical).
    fn active_offset(document: &LynxDocument, swiper: NodeId, vertical: bool) -> f32 {
        let property = if vertical {
            "background-position-y"
        } else {
            "background-position-x"
        };
        let positions = value(document, indicator(document, swiper), property);
        let (active, inactive) = positions.split_once(", ").expect("two layers");
        assert_eq!(inactive, "0px", "the inactive layer stays");
        active
            .strip_suffix("px")
            .and_then(|px| px.parse().ok())
            .unwrap_or_else(|| panic!("a px offset: {positions}"))
    }

    /// At rest on each page of five, the active layer sits on that page's
    /// dot, `k` pitches along, in every mode whose scroll range ends on the
    /// last item's snap position, both ways.
    #[test]
    fn the_active_dot_sits_on_the_current_pages_dot() {
        const PITCH: f32 = 13.44;
        for vertical in [false, true] {
            for mode in ["normal", "carousel", "flat-coverflow"] {
                let (mut document, swiper, _) = mode_swiper(mode, vertical, 5);
                document.commit();
                let content = scroller(&document, swiper);
                let snaps = snap_points(&document, swiper, vertical);
                assert_eq!(snaps.len(), 5, "{mode}");
                for (k, snap) in snaps.into_iter().enumerate() {
                    let to = if vertical {
                        Vector2D::new(0.0, snap)
                    } else {
                        Vector2D::new(snap, 0.0)
                    };
                    document.scroll_to(content, to);
                    document.advance_scroll_timelines(&[content]);
                    document.commit();
                    #[expect(clippy::cast_precision_loss, reason = "k < 5")]
                    let expected = k as f32 * PITCH;
                    let at = active_offset(&document, swiper, vertical);
                    assert!(
                        (at - expected).abs() < 0.01,
                        "{mode} vertical={vertical} page {k}: {at} != {expected}"
                    );
                }
            }
        }
    }

    /// `item`'s own snap position on the swiper's main axis. Under
    /// `circular` it may lie across the seam, past `max_offset`.
    fn snap_position(document: &LynxDocument, swiper: NodeId, item: NodeId, vertical: bool) -> f32 {
        let positions = document
            .snap_positions(scroller(document, swiper))
            .expect("a snapping swiper");
        let axis = if vertical {
            positions.y().expect("snaps on y")
        } else {
            positions.x().expect("snaps on x")
        };
        axis.points
            .iter()
            .find(|point| point.area == item)
            .expect("every item is a snap area")
            .min
    }

    /// A swiper whose main-axis size changes stays on the item it showed
    /// (css-scroll-snap-1 §4.1.3): every item's snap position moves with
    /// the size, and the commit that lays the new size out puts `#content`
    /// on the same item's new position, in every mode, both ways, bounded
    /// and `circular` — where the period changes with the size too, and an
    /// end item's position lies across the seam. The active dot stays on
    /// that item's dot: the strip's timeline is sampled again from the
    /// offset the commit settled on.
    #[test]
    fn a_resized_swiper_stays_on_the_item_it_showed() {
        const PITCH: f32 = 13.44;
        for case in &GEOMETRY {
            for circular in [false, true] {
                for (index, moves) in [(0, false), (1, true), (2, true)] {
                    let (mut document, swiper, items) = mode_swiper(case.mode, case.vertical, 3);
                    if circular {
                        document.set_attribute(swiper, "circular", "");
                    }
                    document.commit();
                    let content = scroller(&document, swiper);
                    let label = format!(
                        "{} vertical={} circular={circular} item {index}",
                        case.mode, case.vertical
                    );
                    // As main adopts the painter's post once the swiper has
                    // come to rest on the item: no request.
                    let before = snap_position(&document, swiper, items[index], case.vertical);
                    document.scroll_to(content, main_axis(case.vertical, before));
                    document.advance_scroll_timelines(&[content]);

                    document.set_inline_style(
                        swiper,
                        if case.vertical {
                            "width: 100px; height: 300px"
                        } else {
                            "width: 300px; height: 100px"
                        },
                    );
                    document.commit();
                    let after = snap_position(&document, swiper, items[index], case.vertical);
                    if moves && !circular {
                        assert!(after > before, "the position moved: {label}");
                    }
                    assert_eq!(
                        document.scroll_offset(content),
                        main_axis(case.vertical, after),
                        "{label}"
                    );
                    if matches!(case.mode, "normal" | "carousel" | "flat-coverflow") {
                        #[expect(clippy::cast_precision_loss, reason = "index < 3")]
                        let dot = index as f32 * PITCH;
                        let at = active_offset(&document, swiper, case.vertical);
                        assert!((at - dot).abs() < 0.01, "the active dot, {at}: {label}");
                    }
                }
            }
        }
    }

    /// The strip's animation is sampled on the main thread — it animates a
    /// background, which no curve exports — and nothing ticks a clock. A
    /// hidden strip runs no animation at all: `display: none` alone would not
    /// stop one here (Stylo's Servo half starts animations on a `display:
    /// none` element), so the hiding rule also drops `animation-name`.
    #[test]
    fn a_hidden_strip_samples_nothing() {
        for hidden in [false, true] {
            let (mut document, swiper, _) = mode_swiper("normal", false, 3);
            if hidden {
                document.set_attribute(swiper, "indicator-dots", "false");
            }
            let frame = document.commit();
            assert!(frame.animation_slots().is_empty(), "hidden={hidden}");
            assert!(!frame.needs_main_ticks(), "hidden={hidden}");
            let content = scroller(&document, swiper);
            document.scroll_to(content, Vector2D::new(400.0, 0.0));
            document.advance_scroll_timelines(&[content]);
            document.commit();
            let expected = if hidden { 0.0 } else { 2.0 * 13.44 };
            assert!(
                (active_offset(&document, swiper, false) - expected).abs() < 0.01,
                "hidden={hidden}"
            );
        }
    }

    // --- `scrollTo` ---------------------------------------------------------

    /// `offset` on the swiper's main axis, the cross axis 0.
    fn main_axis(vertical: bool, offset: f32) -> Vector2D<f32> {
        if vertical {
            Vector2D::new(0.0, offset)
        } else {
            Vector2D::new(offset, 0.0)
        }
    }

    /// A laid-out `normal` row of `count` 200px items in a 200px swiper.
    fn row(count: usize) -> (LynxDocument, NodeId, Vec<NodeId>) {
        let (mut document, swiper, items) = mode_swiper("normal", false, count);
        document.layout();
        (document, swiper, items)
    }

    /// An instant `scrollTo` puts the item on its snap position — its start,
    /// or its centre in the three centred modes — in every mode and
    /// orientation, and the painter's at-rest snap rule leaves it there.
    #[test]
    fn scroll_to_lands_on_the_items_snap_position_in_every_mode() {
        for case in &GEOMETRY {
            for index in [1, 2] {
                let (mut document, swiper, _) = mode_swiper(case.mode, case.vertical, 3);
                document.layout();
                let params = format!(r#"{{"index":{index},"smooth":false}}"#);
                let label = format!("{} vertical={} index={index}", case.mode, case.vertical);
                assert_eq!(scroll_to(&mut document, swiper, &params), Ok(()), "{label}");
                let content = scroller(&document, swiper);
                let expected = main_axis(case.vertical, case.snaps[index]);
                assert_eq!(document.scroll_offset(content), expected, "{label}");
                assert_eq!(
                    document.settle_scroll(content, expected),
                    expected,
                    "already on a snap position: {label}"
                );
            }
        }
    }

    /// Smooth by default: a request the painter carries out, with the
    /// document's offset where it was.
    #[test]
    fn scroll_to_is_smooth_by_default() {
        let (mut document, swiper, _) = row(3);
        assert_eq!(scroll_to(&mut document, swiper, r#"{"index":2}"#), Ok(()));
        assert_eq!(
            document.scroll_offset(scroller(&document, swiper)),
            Vector2D::zero(),
            "the painter moves it"
        );
        assert_eq!(
            pending(&mut document, swiper),
            Some((Vector2D::new(400.0, 0.0), ScrollBehavior::Smooth))
        );
    }

    #[test]
    fn scroll_to_reads_smooth_with_javascript_truthiness() {
        for (smooth, behavior) in [
            ("true", ScrollBehavior::Smooth),
            ("1", ScrollBehavior::Smooth),
            ("\"no\"", ScrollBehavior::Smooth),
            ("{}", ScrollBehavior::Smooth),
            ("false", ScrollBehavior::Instant),
            ("0", ScrollBehavior::Instant),
            ("\"\"", ScrollBehavior::Instant),
            ("null", ScrollBehavior::Instant),
        ] {
            let (mut document, swiper, _) = row(3);
            let params = format!(r#"{{"index":1,"smooth":{smooth}}}"#);
            assert_eq!(
                scroll_to(&mut document, swiper, &params),
                Ok(()),
                "{smooth}"
            );
            let instant = behavior == ScrollBehavior::Instant;
            assert_eq!(
                document.scroll_offset(scroller(&document, swiper)),
                if instant {
                    Vector2D::new(200.0, 0.0)
                } else {
                    Vector2D::zero()
                },
                "{smooth}"
            );
            assert_eq!(
                pending(&mut document, swiper),
                Some((Vector2D::new(200.0, 0.0), behavior)),
                "{smooth}"
            );
        }
    }

    /// A fraction is truncated toward zero; an index naming no item, past
    /// the end or below zero, is code 4 with nothing moved and nothing
    /// recorded.
    #[test]
    fn scroll_to_truncates_a_fraction_and_refuses_an_index_naming_no_item() {
        for (index, expected) in [("1.9", 200.0), ("-0.5", 0.0)] {
            let (mut document, swiper, _) = row(3);
            let content = scroller(&document, swiper);
            document.scroll_to(content, Vector2D::new(400.0, 0.0));
            let params = format!(r#"{{"index":{index},"smooth":false}}"#);
            assert_eq!(scroll_to(&mut document, swiper, &params), Ok(()), "{index}");
            assert_eq!(
                document.scroll_offset(content),
                Vector2D::new(expected, 0.0),
                "{index}"
            );
        }
        for index in ["3", "9", "-1", "1e308", "-1e308"] {
            let (mut document, swiper, _) = row(3);
            let content = scroller(&document, swiper);
            let params = format!(r#"{{"index":{index},"smooth":false}}"#);
            assert_eq!(
                scroll_to(&mut document, swiper, &params),
                Err(InvalidParams),
                "{index}"
            );
            assert_eq!(document.scroll_offset(content), Vector2D::zero(), "{index}");
            assert_eq!(document.pending_scroll_request(content), None, "{index}");
        }
    }

    #[test]
    fn scroll_to_refuses_params_without_a_numeric_index() {
        for params in [
            "{}",
            r#"{"index":"1"}"#,
            r#"{"index":null}"#,
            r#"{"index":true}"#,
            r#"{"smooth":false}"#,
            "null",
            "[1]",
            "1",
            "not json",
        ] {
            let (mut document, swiper, _) = row(3);
            let content = scroller(&document, swiper);
            assert_eq!(
                scroll_to(&mut document, swiper, params),
                Err(InvalidParams),
                "{params}"
            );
            assert_eq!(
                document.scroll_offset(content),
                Vector2D::zero(),
                "{params}"
            );
            assert_eq!(document.pending_scroll_request(content), None, "{params}");
        }
    }

    /// Only element children are items: a text node between two items does
    /// not shift the index.
    #[test]
    fn scroll_to_counts_element_children_only() {
        let mut document = document();
        let (swiper, _) = build(&mut document, "width: 200px; height: 100px", &[], 1);
        let text = document.create_text_node("between", ());
        document.append_child(swiper, text);
        for _ in 0..2 {
            element_under(&mut document, swiper, SWIPER_ITEM_TAG, "");
        }
        document.layout();
        assert_eq!(
            scroll_to(&mut document, swiper, r#"{"index":2,"smooth":false}"#),
            Ok(())
        );
        assert_eq!(
            document.scroll_offset(scroller(&document, swiper)),
            Vector2D::new(400.0, 0.0)
        );
        assert_eq!(
            scroll_to(&mut document, swiper, r#"{"index":3,"smooth":false}"#),
            Err(InvalidParams),
            "three element children, whatever else the swiper holds"
        );
    }

    /// An item's `scroll-margin` moves its snap position, and `scrollTo`
    /// moves with it, at the start and at the centre alike. The modes are the
    /// two whose items stay narrower than the port with the margin added, so
    /// each snap position is a single offset.
    #[test]
    fn scroll_to_follows_the_items_scroll_margin() {
        for (mode, unmargined) in [("carousel", 160.0), ("flat-coverflow", 120.0)] {
            let (mut document, swiper, items) = mode_swiper(mode, false, 3);
            document.set_inline_style(items[1], "scroll-margin-left: 10px");
            document.layout();
            let snap = snap_points(&document, swiper, false)[1];
            assert_eq!(
                snap,
                unmargined - if mode == "carousel" { 10.0 } else { 5.0 }
            );
            assert_eq!(
                scroll_to(&mut document, swiper, r#"{"index":1,"smooth":false}"#),
                Ok(()),
                "{mode}"
            );
            assert_eq!(
                document.scroll_offset(scroller(&document, swiper)),
                Vector2D::new(snap, 0.0),
                "{mode}"
            );
        }
    }

    /// The item's own `scroll-snap-align` decides, an author's included:
    /// `end` puts its end edge at the scrollport's end, and `none` (no snap
    /// area) its start at the start. In `flat-coverflow` item 1 is laid out
    /// at 160 to 280 in a 200px port.
    #[test]
    fn scroll_to_aligns_as_the_items_own_scroll_snap_align() {
        for (align, expected) in [("none", 160.0), ("end", 80.0), ("center", 120.0)] {
            let (mut document, swiper, items) = mode_swiper("flat-coverflow", false, 3);
            document.set_inline_style(items[1], &format!("scroll-snap-align: {align}"));
            document.layout();
            assert_eq!(rect(&document, items[1]), (160.0, 0.0, 120.0, 100.0));
            assert_eq!(
                scroll_to(&mut document, swiper, r#"{"index":1,"smooth":false}"#),
                Ok(()),
                "{align}"
            );
            assert_eq!(
                document.scroll_offset(scroller(&document, swiper)),
                Vector2D::new(expected, 0.0),
                "{align}"
            );
        }
    }

    /// A `display: none` swiper gives its items no box, so there is nothing
    /// to scroll: the call succeeds and nothing moves or is recorded.
    #[test]
    fn scroll_to_on_a_swiper_with_no_box_moves_nothing() {
        let mut document = document();
        let (swiper, _) = build(&mut document, "display: none", &[], 3);
        document.layout();
        let content = scroller(&document, swiper);
        assert_eq!(
            scroll_to(&mut document, swiper, r#"{"index":1,"smooth":false}"#),
            Ok(())
        );
        assert_eq!(document.scroll_offset(content), Vector2D::zero());
        assert_eq!(document.pending_scroll_request(content), None);
    }

    /// A `wrapper` child is an element child, so `index` counts it, but it is
    /// `display: contents` and has no box of its own: naming it succeeds with
    /// nothing moved. The item after it is the next index.
    #[test]
    fn scroll_to_counts_a_wrapper_which_has_no_box_to_scroll_to() {
        let mut document = document();
        let (swiper, _) = build(&mut document, "width: 200px; height: 100px", &[], 1);
        let wrapper = element_under(&mut document, swiper, "wrapper", "");
        element_under(&mut document, wrapper, SWIPER_ITEM_TAG, "");
        let last = element_under(&mut document, swiper, SWIPER_ITEM_TAG, "");
        document.layout();
        let content = scroller(&document, swiper);
        assert_eq!(
            scroll_to(&mut document, swiper, r#"{"index":1,"smooth":false}"#),
            Ok(())
        );
        assert_eq!(document.scroll_offset(content), Vector2D::zero());
        assert_eq!(document.pending_scroll_request(content), None);

        let (start, ..) = rect(&document, last);
        assert!(start > 0.0, "the last item lies past the first");
        assert_eq!(
            scroll_to(&mut document, swiper, r#"{"index":2,"smooth":false}"#),
            Ok(())
        );
        assert_eq!(document.scroll_offset(content), Vector2D::new(start, 0.0));
    }

    /// `invoke` answers `scrollTo` alone; bad params are code 4, and any
    /// other name is the kind's `NotFound`, which leaves it to the base set.
    #[test]
    fn invoke_answers_scroll_to_and_nothing_else() {
        let (mut document, swiper, _) = row(3);
        let mut invoke = |name: &str, params: &str| {
            document.invoke_element_method(swiper, MethodCall { name, params })
        };
        assert_eq!(
            invoke("scrollTo", r#"{"index":1,"smooth":false}"#),
            MethodOutcome::Done
        );
        assert_eq!(
            invoke("scrollTo", r#"{"index":3}"#),
            MethodOutcome::Failed(MethodError::InvalidParams)
        );
        assert_eq!(
            invoke("scrollTo", "{}"),
            MethodOutcome::Failed(MethodError::InvalidParams)
        );
        for name in ["selectTab", "scrollBy", "scrollToNext", "getScrollInfo"] {
            assert_eq!(
                invoke(name, r#"{"index":1}"#),
                MethodOutcome::NotFound,
                "{name}"
            );
        }
        assert_eq!(
            document.scroll_offset(scroller(&document, swiper)),
            Vector2D::new(200.0, 0.0)
        );
    }

    // --- autoplay -----------------------------------------------------------

    /// One autoplay turn of `swiper`, as far as the painter: the request the
    /// next commit carries for it — target, behavior, and whether it yields
    /// to a pointer — then carried out and acknowledged as the painter's
    /// post would, so the next turn counts from the new item. `None` when
    /// the turn asked for nothing.
    fn turned(
        document: &mut LynxDocument,
        swiper: NodeId,
    ) -> Option<(Vector2D<f32>, ScrollBehavior, bool)> {
        super::turn(document, swiper);
        let content = scroller(document, swiper);
        // Asked first: a frame goes on carrying an acknowledged request
        // until something else commits.
        document.pending_scroll_request(content)?;
        let frame = document.commit();
        let index = frame
            .slot_of(content)
            .expect("the swiper's scroller has a slot");
        let request = frame.scroll_slots()[index as usize].request?;
        document.scroll_to(content, request.target);
        document.acknowledge_scroll_request(content, request.serial);
        Some((request.target, request.behavior, request.yields_to_pointer))
    }

    /// Each turn asks for the next item's snap position, smoothly and as a
    /// request that yields to a pointer, in every mode and both
    /// orientations; on the last item it asks for nothing.
    #[test]
    fn a_turn_goes_to_the_next_items_snap_position_and_stays_on_the_last() {
        for case in &GEOMETRY {
            let (mut document, swiper, _) = mode_swiper(case.mode, case.vertical, 3);
            document.commit();
            let [_, second, third] = case.snaps;
            let label = format!("{} vertical={}", case.mode, case.vertical);
            assert_eq!(
                turned(&mut document, swiper),
                Some((
                    main_axis(case.vertical, second),
                    ScrollBehavior::Smooth,
                    true
                )),
                "{label}"
            );
            assert_eq!(
                turned(&mut document, swiper),
                Some((
                    main_axis(case.vertical, third),
                    ScrollBehavior::Smooth,
                    true
                )),
                "{label}"
            );
            assert_eq!(turned(&mut document, swiper), None, "{label}: the last");
        }
    }

    #[test]
    fn a_turn_wraps_from_the_last_item_only_when_circular() {
        for (circular, expected) in [("", Some(0.0)), ("true", Some(0.0)), ("false", None)] {
            let mut document = document();
            let (swiper, _) = build(
                &mut document,
                "width: 200px; height: 100px",
                &[("circular", circular)],
                3,
            );
            document.commit();
            document.scroll_to(scroller(&document, swiper), Vector2D::new(400.0, 0.0));
            assert_eq!(
                turned(&mut document, swiper).map(|(target, ..)| target.x),
                expected,
                "circular={circular:?}"
            );
        }
    }

    /// The current item is the one whose snap position is nearest the
    /// offset, the first on a tie: a swiper left between two items counts
    /// from the nearer.
    #[test]
    fn a_turn_counts_from_the_nearest_item() {
        for (from, to) in [(90.0, 200.0), (110.0, 400.0), (100.0, 200.0)] {
            let (mut document, swiper, _) = row(3);
            document.scroll_to(scroller(&document, swiper), Vector2D::new(from, 0.0));
            assert_eq!(
                turned(&mut document, swiper).map(|(target, ..)| target.x),
                Some(to),
                "from {from}"
            );
        }
    }

    /// `smooth-scroll` present with any value makes the turn instant,
    /// web-core's reading. The document's own offset waits for the painter
    /// either way: only the painter knows whether a finger had the swiper.
    #[test]
    fn smooth_scroll_makes_the_turn_instant_and_the_document_still_waits() {
        for value in ["", "true", "false"] {
            let (mut document, swiper, _) = row(3);
            document.set_attribute(swiper, "smooth-scroll", value);
            document.commit();
            super::turn(&mut document, swiper);
            assert_eq!(
                document.scroll_offset(scroller(&document, swiper)),
                Vector2D::zero(),
                "{value:?}: not moved by the document"
            );
            assert_eq!(
                pending(&mut document, swiper),
                Some((Vector2D::new(200.0, 0.0), ScrollBehavior::Instant)),
                "{value:?}"
            );
        }
    }

    /// A `scrollTo` a script made and the painter has not handled yet is not
    /// replaced by a turn.
    #[test]
    fn a_turn_does_not_replace_a_scroll_a_script_asked_for() {
        let (mut document, swiper, _) = row(3);
        scroll_to(&mut document, swiper, r#"{"index":2}"#).expect("valid params");
        super::turn(&mut document, swiper);
        assert_eq!(
            pending(&mut document, swiper),
            Some((Vector2D::new(400.0, 0.0), ScrollBehavior::Smooth)),
            "the script's turn to the last item stands"
        );
    }

    #[test]
    fn a_swiper_with_no_box_or_no_items_turns_nothing() {
        let (mut document, swiper, _) = row(3);
        document.set_inline_style(swiper, "display: none");
        document.commit();
        super::turn(&mut document, swiper);
        let content = scroller(&document, swiper);
        assert_eq!(document.pending_scroll_request(content), None);

        let (mut document, swiper, _) = row(0);
        super::turn(&mut document, swiper);
        let content = scroller(&document, swiper);
        assert_eq!(document.pending_scroll_request(content), None);
    }

    /// `interval` is `parseFloat`'s reading in milliseconds, 5000 when it is
    /// missing or no number, and no shorter than a nested timer's 4 ms.
    #[test]
    fn the_period_is_the_interval_in_milliseconds() {
        for (interval, milliseconds) in [
            (None, 5000),
            (Some(""), 5000),
            (Some("fast"), 5000),
            (Some("Infinity"), 5000),
            (Some("1000"), 1000),
            (Some("  250.5"), 250),
            (Some("2500ms"), 2500),
            (Some("1e3"), 1000),
            (Some("12e"), 12),
            (Some("0"), 4),
            (Some("-300"), 4),
            (Some("1e300"), 2_147_483_647),
        ] {
            assert_eq!(
                autoplay_period(interval).as_millis(),
                milliseconds,
                "{interval:?}"
            );
        }
    }

    /// How long from now `autoplays`' earliest turn is due, in milliseconds.
    fn due_in(autoplays: &Autoplays) -> Option<u128> {
        autoplays
            .next_deadline()
            .map(|deadline| deadline.duration_since(ClockInstant::now()).as_millis())
    }

    /// A detached `x-swiper` with `attributes`, in a document whose autoplay
    /// timers the test holds.
    fn detached_swiper(attributes: &[(&str, &str)]) -> (LynxDocument, Autoplays, NodeId) {
        let (mut document, autoplays) = with_autoplays();
        let swiper = document.create_element(SWIPER_TAG, ());
        for (name, value) in attributes {
            document.set_attribute(swiper, name, value);
        }
        (document, autoplays, swiper)
    }

    /// The timer is a connected swiper's: it starts on connection, ends on
    /// disconnection, and a new one starts on the next connection.
    #[test]
    fn autoplay_runs_from_connection_to_disconnection() {
        let (mut document, autoplays, swiper) = detached_swiper(&[("autoplay", "")]);
        assert_eq!(due_in(&autoplays), None, "not connected yet");
        let page = document.document_element().id();
        document.append_child(page, swiper);
        let due = due_in(&autoplays).expect("started on connection");
        assert!((4900..=5000).contains(&due), "one interval away, got {due}");

        document.remove_element(swiper);
        assert_eq!(due_in(&autoplays), None, "ended on disconnection");
        document.append_child(page, swiper);
        assert!(due_in(&autoplays).is_some(), "and started again");

        document.drop_subtree(swiper);
        assert_eq!(due_in(&autoplays), None, "a freed swiper keeps none");
    }

    #[test]
    fn a_swiper_without_autoplay_keeps_no_timer() {
        for attributes in [&[][..], &[("autoplay", "false")], &[("interval", "100")]] {
            let (mut document, autoplays, swiper) = detached_swiper(attributes);
            let page = document.document_element().id();
            document.append_child(page, swiper);
            assert_eq!(due_in(&autoplays), None, "{attributes:?}");
        }
    }

    /// `autoplay` or `interval` written on a connected swiper that has
    /// `autoplay` starts the timer over at the period `interval` names.
    #[test]
    fn a_change_of_autoplay_or_interval_starts_the_timer_over() {
        let (mut document, autoplays, swiper) = detached_swiper(&[]);
        let page = document.document_element().id();
        document.append_child(page, swiper);
        document.set_attribute(swiper, "interval", "60000");
        assert_eq!(due_in(&autoplays), None, "no autoplay yet");

        document.set_attribute(swiper, "autoplay", "true");
        let due = due_in(&autoplays).expect("started");
        assert!((59_900..=60_000).contains(&due), "got {due}");

        document.set_attribute(swiper, "interval", "1000");
        let due = due_in(&autoplays).expect("started over");
        assert!((900..=1000).contains(&due), "the new period, got {due}");

        document.remove_attribute(swiper, "interval");
        let due = due_in(&autoplays).expect("started over");
        assert!((4900..=5000).contains(&due), "the default, got {due}");
    }

    /// web-core's handler has no branch for `autoplay` going away, so the
    /// timer it started keeps running at the period it has, whatever
    /// `interval` is written afterwards.
    #[test]
    fn removing_autoplay_leaves_the_timer_running() {
        for stop in [None, Some("false")] {
            let (mut document, autoplays, swiper) =
                detached_swiper(&[("autoplay", ""), ("interval", "60000")]);
            let page = document.document_element().id();
            document.append_child(page, swiper);
            let armed = autoplays.next_deadline().expect("started");

            match stop {
                None => document.remove_attribute(swiper, "autoplay"),
                Some(value) => document.set_attribute(swiper, "autoplay", value),
            }
            assert_eq!(autoplays.next_deadline(), Some(armed), "{stop:?}: running");
            document.set_attribute(swiper, "interval", "100");
            assert_eq!(
                autoplays.next_deadline(),
                Some(armed),
                "{stop:?}: and at its old period"
            );
        }
    }

    /// What came due is turned and armed again one period after the run;
    /// what has not is left alone.
    #[test]
    fn a_run_turns_what_is_due_and_arms_the_next_turn() {
        let (mut document, autoplays) = with_autoplays();
        let (swiper, _) = build(
            &mut document,
            "width: 200px; height: 100px",
            &[("autoplay", ""), ("interval", "1000")],
            3,
        );
        document.commit();
        let content = scroller(&document, swiper);
        let armed = autoplays.next_deadline().expect("started");

        let early = armed
            .checked_sub(Duration::from_millis(1))
            .expect("a deadline a second away has a millisecond before it");
        autoplays.run_due(&mut document, early);
        assert_eq!(document.pending_scroll_request(content), None, "not due");
        assert_eq!(autoplays.next_deadline(), Some(armed));

        let late = armed + Duration::from_millis(30);
        autoplays.run_due(&mut document, late);
        assert_eq!(
            pending(&mut document, swiper),
            Some((Vector2D::new(200.0, 0.0), ScrollBehavior::Smooth)),
            "turned to the second item"
        );
        assert_eq!(
            autoplays.next_deadline(),
            Some(late + Duration::from_secs(1)),
            "one period after the run"
        );
    }
}
