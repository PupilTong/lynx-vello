//! The `x-swiper` and `x-swiper-item` tags: a row (or, with `vertical`, a
//! column) of items the user swipes through one item at a time, in one of
//! five layouts, with a strip of dots and an optional autoplay timer.
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
//! "present and not `"false"`", and `smooth-scroll`, `autoplay` and
//! `indicator-dots` are read below as web-core reads them.
//!
//! # What the attributes do
//!
//! - `vertical`: the column. Overflow axes swap, `scroll-snap-type: y mandatory`
//!   (`x-swiper.css:55-62`).
//! - `mode`: `normal` (default) — items 100% of the main axis, start-aligned. `carousel` — items
//!   80%, start-aligned, the last item followed by a 20% margin so it can reach its start
//!   (`:181-195`). `flat-coverflow` — items 60%, centre-aligned, 20% margin before the first and
//!   after the last (`:271-297`). `coverflow` — `flat-coverflow`'s geometry plus a scroll-driven
//!   scale (`:205-267`). `carry` — items 100%, centre-aligned, plus a scroll-driven scale
//!   (`:299-317`). Each margin sits on the main axis; web-core writes the carousel's as
//!   `margin-right` in both orientations (`:193-195`), here it is `margin-bottom` under `vertical`.
//!   A percentage margin resolves against `#content`'s width on every side, as CSS resolves it, so
//!   under `vertical` the 20% is of the width, as in web-core; native offsets by 20% of the main
//!   axis (`XSwiperUI.java:582-604`).
//! - `current`: the item the swiper starts on. See below.
//! - `bounces` (present and not `"false"`): `overscroll-behavior` `contain-bounce` on `#content`'s
//!   main axis, as for [`super::viewpager`]. web-core shows a page-wide blank box ahead of the
//!   items (`x-swiper.css:64-66`, `htmlTemplates.ts:226-232`), so only the leading edge can be
//!   pulled.
//! - `indicator-dots`, `indicator-color`, `indicator-active-color`: the strip, below.
//! - `autoplay`, `interval`, `smooth-scroll`, `circular`: autoplay, below.
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
//! running web-core's rules.
//!
//! # The two animated modes are scroll-driven, and flat
//!
//! `coverflow` and `carry` scale each item by where it stands in the scrollport:
//! `animation-timeline: view(inline)` (`view(block)` under `vertical`; this engine's `inline` is
//! x), `animation-duration: auto`, `animation-fill-mode: both`, linear timing. `coverflow` runs
//! web-core's keyframes without their `rotateY`/`rotateX` and without `perspective`,
//! `transform-style` and `z-index`: `25%` `scale(0.8)`, `45%, 55%` `scale(1)`, `100%`
//! `scale(0.8)`. `carry` runs `0%` `scale(0.6)`, `45%, 55%` `scale(1)`, `100%` `scale(0.6)`.
//! Dropping the rotation is deliberate — a 3D transform keyframe refuses the curve export
//! (`crates/dom/src/style/curve_export.rs`), and then every item would re-cascade on the main
//! thread at every scroll frame. Flat, both export: the painter samples them from `#content`'s
//! live offset, with no main-thread tick and no commit while the user drags. `fill-mode: both` is
//! what lets them export at any committed offset (`docs/tracking/css-animation.md`, "Base-value
//! rule"). web-core leaves the timing at `ease` per segment; linear is this engine's choice.
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
//! where web-core lands too: its `#scrollToIndex` (`XSwiper.ts:104-129`) aims at the centre in
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
//! the number of its element children, all of them, as web-core's `childElementCount` counts them,
//! through a presentational hint it rewrites in
//! [`CustomElement::children_changed`](dom::CustomElement::children_changed) — the standard's
//! `MutationObserver` on the host's child list, which web-core attaches for the same purpose
//! (`:103-113`).
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
//!   `count` steps, whose held value is `k × pitch`. web-core instead runs one animation per dot on
//!   that item's view timeline, active between 30% and 70% of it (`htmlTemplates.ts:253-266`).
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
//! # Autoplay
//!
//! web-core's `XSwiperAutoScroll.ts`. `autoplay` present (and not `"false"`) runs an interval of
//! `interval` ms — `parseFloat`, `5000` when missing or `NaN` (`:60-70`); changing either
//! attribute restarts it. Removing `autoplay` stops it, which is native's behaviour (Android
//! `XSwiperUI.java:684-691`): web-core's handler only ever starts an interval, so its autoplay
//! never stops (`XSwiperAutoScroll.ts:60-70`). The interval lives in the realm
//! (`packages/bobcat-element/src/element-papi.ts`, `syncAutoplay`), holds the element's handle
//! weakly, and clears itself the first tick after the handle is collected. Each tick calls
//! [`advance`], which turns `#content` to the next item, or from the last one to the first under
//! `circular`.
//!
//! # What is not implemented
//!
//! - `circular` wrap-around dragging: web-core re-slots the first and last items into its shadow
//!   tree around the current one (`XSwiperCircular.ts`) and turns snapping off
//!   (`x-swiper.css:122-131`). Here a `circular` swiper drags as a plain one; `circular` only makes
//!   autoplay wrap.
//! - The 3D rotation of `coverflow`, above.
//! - `page-margin`, `previous-margin`, `next-margin` and `duration` have no rule, following
//!   web-core, where they change nothing on screen: the three margins become custom properties on
//!   the indicator container (`XSwiperIndicator.ts:54-74`), so the items always see `0px`, and
//!   nothing observes `duration`. Native differs: `page-margin` is the gap between pages
//!   (`XSwiperUI.java:569`), `previous-margin`/`next-margin` size and offset the page in
//!   `coverflow`, `flat-coverflow` and `carry` (`:617-638`), and `duration` is the turn's animation
//!   length, 500 ms by default (`:91,855-863`).
//! - Every event (`change`, `scrollstart`, `scrollend`, `transition`) and every UI method.
//! - `contain: strict` and `content-visibility: auto` from the twentieth item
//!   (`x-swiper.css:77-80`), `scrollbar-width` and the per-item `view-timeline-name` rules: a
//!   browser shortcut, a scrollbar this engine does not draw, and web-core's per-dot timelines.

use dom::scroll::ScrollBehavior;
use dom::{CustomElement, Node, NodeId, ShadowRootMode, Vector2D};

use super::LynxDocument;

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
x-swiper:is([mode="flat-coverflow"], [mode="coverflow"]):is(:not([vertical]), [vertical="false"])
  > x-swiper-item:first-child { margin-left: 20%; }
x-swiper:is([mode="flat-coverflow"], [mode="coverflow"])[vertical]:not([vertical="false"])
  > x-swiper-item:first-child { margin-top: 20%; }
x-swiper:is([mode="carousel"], [mode="flat-coverflow"], [mode="coverflow"]):is(:not([vertical]), [vertical="false"])
  > x-swiper-item:last-child { margin-right: 20%; }
x-swiper:is([mode="carousel"], [mode="flat-coverflow"], [mode="coverflow"])[vertical]:not([vertical="false"])
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
x-swiper[mode="coverflow"] > x-swiper-item { animation-name: x-swiper-coverflow; }
x-swiper[mode="carry"] > x-swiper-item { animation-name: x-swiper-carry; }
@keyframes x-swiper-coverflow {
  25% { transform: scale(0.8); }
  45%, 55% { transform: scale(1); }
  100% { transform: scale(0.8); }
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
pub(super) fn define(document: &mut LynxDocument) {
    document.define(SWIPER_TAG, Box::new(Swiper));
}

/// The `x-swiper` component: builds the shadow tree when the element is
/// constructed and keeps `--swiper-count` equal to the number of its
/// element children.
///
/// The shadow tree is built in `constructed`, not on connection: the element
/// is new and detached then, and `__CreateElement` mints a swiper before
/// anything is appended to it, so every child arrives through
/// `children_changed`.
struct Swiper;

impl CustomElement<()> for Swiper {
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

    fn children_changed(&self, document: &mut LynxDocument, element: NodeId) {
        count_items(document, element);
    }
}

/// Writes the number of `swiper`'s element children, every one of them as
/// web-core's `childElementCount` counts them, into its `--swiper-count`
/// hint.
fn count_items(document: &mut LynxDocument, swiper: NodeId) {
    let count = document.get(swiper).map_or(0, |node| {
        node.children().filter(|child| child.is_element()).count()
    });
    document.set_presentational_hint(swiper, COUNT_PROPERTY, &count.to_string());
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

/// Whether `element` carries the boolean attribute `name`: present and not
/// `"false"`, the attribute web-core's false-filtering leaves.
fn flag(element: &Node<()>, name: &str) -> bool {
    element
        .attribute(name)
        .is_some_and(|value| value != "false")
}

/// One autoplay tick on `swiper`: turns it to the item after the current
/// one, or from the last item to the first when it is `circular`.
///
/// The positions are the swiper's snap positions on its main axis
/// ([`dom::Document::snap_positions`]), one per item, in order. The current
/// item is the one whose position is nearest the offset, the first on a tie.
/// That is web-core's "item whose centre is nearest the scrollport's mid"
/// (`XSwiper.ts:37-91`, the mid at 40% of the scrollport in `carousel`)
/// restated: items are equal on the main axis, and each one's position is
/// its centre less the same distance — half the scrollport for a centred
/// item, half the item for a start-aligned one, which in `carousel` is the
/// 40% web-core uses. The target is that item's snap position, which is
/// where web-core's `#scrollToIndex` ends after its mandatory snap
/// (`:104-129`); on the last item without `circular` nothing moves
/// (`XSwiperAutoScroll.ts:22-33`).
///
/// The turn is smooth unless `smooth-scroll` is present with any value,
/// `"false"` included, which is web-core's reading
/// (`getAttribute('smooth-scroll') === null`, `XSwiper.ts:101`, with
/// `smooth-scroll` exempt from its false-filtering, `:28-31`). Native reads
/// the attribute as a boolean that defaults to `true`
/// (`XSwiperUI.java:877-885`). A smooth turn leaves the document's offset where it was until the
/// painter posts it back, so a tick that comes before then counts from the
/// old item and asks for the same one again.
///
/// The offset, the positions and the turn are `#content`'s ([`content`]),
/// the scroll container. Reads the last completed layout and never flushes.
/// A swiper with no box, no snap positions or no items, or any other
/// element, moves nothing.
pub(crate) fn advance(document: &mut LynxDocument, swiper: NodeId) {
    let Some(element) = document.get(swiper) else {
        return;
    };
    if element.tag_name() != Some(SWIPER_TAG) {
        return;
    }
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
    document.scroll_to_with(content, to, behavior);
}

#[cfg(test)]
mod tests {
    #![allow(clippy::float_cmp)] // Explicit pixel sizes lay out exactly.

    use dom::scroll::ScrollBehavior;
    use dom::stylo::properties::PropertyId;
    use dom::stylo::values::computed::{Display, Overflow};
    use dom::{NodeId, StylesheetOrigin, Vector2D};

    use super::super::LynxDocument;
    use super::super::test_support::{
        child, display, document, document as fresh, element_under, overflow, style_of,
    };
    use super::{SWIPER_ITEM_TAG, SWIPER_TAG, advance};

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
        assert_eq!(style_of(&document, swiper).clone_display(), Display::Linear);

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
        assert_eq!(style_of(&document, content).clone_display(), Display::Flex);
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
        assert_eq!(style_of(&document, view).clone_display(), Display::None);
        assert_eq!(
            style_of(&document, wrapper).clone_display(),
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
        assert_eq!(style_of(&document, item).clone_display(), Display::Linear);
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

    /// `coverflow` and `carry` export their scale to the painter as a curve
    /// that reads the swiper's scroll slot: nothing ticks on the main
    /// thread and no curve reads the clock. `normal`, `carousel` and
    /// `flat-coverflow` animate nothing.
    #[test]
    fn the_animated_modes_export_a_scroll_driven_scale() {
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

    /// `coverflow`'s keyframes carry no rotation: the item beside the
    /// current one is scaled, in 2D.
    #[test]
    fn coverflow_scales_without_rotating() {
        let (mut document, _, items) = mode_swiper("coverflow", false, 3);
        document.commit();
        assert_eq!(value(&document, items[0], "transform"), "scale(1)");
        let beside = value(&document, items[1], "transform");
        assert!(
            beside.starts_with("scale(") && beside != "scale(1)",
            "{beside}"
        );
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

    // --- advance ------------------------------------------------------------

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

    /// A laid-out swiper of three items, `smooth-scroll` set so each turn
    /// moves the document at once.
    fn instant(mode: &str, vertical: bool, attributes: &[(&str, &str)]) -> (LynxDocument, NodeId) {
        let (mut document, swiper, _) = mode_swiper(mode, vertical, 3);
        document.set_attribute(swiper, "smooth-scroll", "");
        for (name, value) in attributes {
            document.set_attribute(swiper, name, value);
        }
        document.commit();
        (document, swiper)
    }

    /// Each tick turns to the next item's snap position, in every mode and
    /// both orientations, and the last item stays.
    #[test]
    fn advance_turns_to_the_next_item_and_stops_at_the_last() {
        for case in &GEOMETRY {
            let (mut document, swiper) = instant(case.mode, case.vertical, &[]);
            let main = |document: &LynxDocument| {
                let offset = document.scroll_offset(scroller(document, swiper));
                if case.vertical { offset.y } else { offset.x }
            };
            let mut seen = vec![main(&document)];
            for _ in 0..3 {
                advance(&mut document, swiper);
                seen.push(main(&document));
            }
            let [first, second, third] = case.snaps;
            assert_eq!(
                seen,
                [first, second, third, third],
                "{} vertical={}",
                case.mode,
                case.vertical
            );
        }
    }

    #[test]
    fn advance_wraps_from_the_last_item_only_when_circular() {
        for (circular, expected) in [("", 0.0), ("true", 0.0), ("false", 400.0)] {
            let (mut document, swiper) = instant("normal", false, &[("circular", circular)]);
            document.scroll_to(scroller(&document, swiper), Vector2D::new(400.0, 0.0));
            advance(&mut document, swiper);
            assert_eq!(
                document.scroll_offset(scroller(&document, swiper)),
                Vector2D::new(expected, 0.0),
                "circular={circular:?}"
            );
        }
    }

    /// The current item is the one whose snap position is nearest the
    /// offset: a swiper the user left between items counts from the nearer.
    #[test]
    fn advance_counts_from_the_nearest_item() {
        for (from, to) in [(90.0, 200.0), (110.0, 400.0), (100.0, 200.0)] {
            let (mut document, swiper) = instant("normal", false, &[]);
            document.scroll_to(scroller(&document, swiper), Vector2D::new(from, 0.0));
            advance(&mut document, swiper);
            assert_eq!(
                document.scroll_offset(scroller(&document, swiper)),
                Vector2D::new(to, 0.0),
                "{from}"
            );
        }
    }

    /// Smooth by default: the request goes to the painter and the document
    /// stays. `smooth-scroll` with any value, `"false"` included, is instant.
    #[test]
    fn advance_is_smooth_unless_smooth_scroll_is_present() {
        for (attribute, behavior) in [
            (None, ScrollBehavior::Smooth),
            (Some(""), ScrollBehavior::Instant),
            (Some("true"), ScrollBehavior::Instant),
            (Some("false"), ScrollBehavior::Instant),
        ] {
            let (mut document, swiper, _) = mode_swiper("normal", false, 3);
            if let Some(attribute) = attribute {
                document.set_attribute(swiper, "smooth-scroll", attribute);
            }
            document.commit();
            advance(&mut document, swiper);
            assert_eq!(
                pending(&mut document, swiper),
                Some((Vector2D::new(200.0, 0.0), behavior)),
                "{attribute:?}"
            );
            let expected = if behavior == ScrollBehavior::Smooth {
                0.0
            } else {
                200.0
            };
            assert_eq!(
                document.scroll_offset(scroller(&document, swiper)),
                Vector2D::new(expected, 0.0)
            );
        }
    }

    /// Before its first layout, without a box, or when it is no swiper,
    /// nothing moves and no request is recorded.
    #[test]
    fn advance_without_a_laid_out_swiper_moves_nothing() {
        let (mut document, swiper, _) = mode_swiper("normal", false, 3);
        advance(&mut document, swiper);
        assert_eq!(
            document.pending_scroll_request(scroller(&document, swiper)),
            None,
            "no layout yet"
        );

        let (mut document, swiper, _) = mode_swiper("normal", false, 0);
        document.commit();
        advance(&mut document, swiper);
        assert_eq!(
            document.pending_scroll_request(scroller(&document, swiper)),
            None,
            "no items"
        );

        let mut document = fresh();
        let (swiper, _) = build(&mut document, "display: none", &[], 3);
        document.commit();
        advance(&mut document, swiper);
        assert_eq!(
            document.pending_scroll_request(scroller(&document, swiper)),
            None,
            "no box"
        );

        let mut document = fresh();
        let scroller = child(
            &mut document,
            "scroll-view",
            "width: 100px; height: 100px; scroll-snap-type: y mandatory",
        );
        for _ in 0..2 {
            element_under(
                &mut document,
                scroller,
                "view",
                "height: 100px; flex-shrink: 0; scroll-snap-align: start",
            );
        }
        document.commit();
        advance(&mut document, scroller);
        assert_eq!(document.pending_scroll_request(scroller), None, "no swiper");
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

    /// `--swiper-count` is the host's element children, every one of them:
    /// a child that is no item counts, a text node does not.
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
}
