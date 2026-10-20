# Layout events: `layoutchange`, `<text>` `layout`, and a ResizeObserver primitive

Investigation and design evaluation, 2026-10-20. The decisions are listed
at the end, with what was built from them; the rest is what the references
do, what this engine already has, and what each implementation path costs.

## 1. Which layout events Lynx has

### 1.1 `layoutchange` (`bindlayoutchange`), every element

**Native** (`lynx/`). Every `LynxUI`/`LynxBaseUI` sends it from the frame
update the UI owner applies after a layout pass
(`LynxUI.m:429` `sendLayoutChangeEvent` at the end of `updateFrame:`,
`LynxBaseUI.java:2584` at the end of `updateLayout`), when the element has a
`layoutchange` entry in its event set. The UI owner applies a frame only to
the UIs whose starlight node reports `GetHasNewLayout()`
(`element.cc:2940`), and starlight marks that when, after pixel rounding,
the node's result differs from the last one in **size, offset within its
parent, padding, border, margin or sticky positions**, on the first layout,
or when a measure-function node (text, image) was dirty and re-measured
(`layout_object.cc:275-279`, `SetNewLayoutResult` `:300-322`). A child
whose parent moved keeps its offset within the parent, so an ancestor's
move does not fire the event on its descendants.

Detail (`LynxUI.m:3790-3802`, `LynxBaseUI.java:1023-1043`):
`{id, dataset, left, top, right, bottom, width, height}`, the element's
rect relative to the root UI: iOS `convertRect:toView:rootView`
(transforms included), Android `getBoundingClientRect` or, under the env
flag `enableTransformForPositionCalculation`, a transform-aware conversion.
Types: `LayoutChangeDetailEvent` (`js_libraries/types/types/common/events.d.ts:419`).

**web-core** (`lynx-stack/`). `CommonEventsAndMethods.ts:23-58`: while the
element has a `layoutchange` listener (the `registerEventEnableStatusChangeHandler`
switch, the same mechanism `ELEMENT_REACTIVE_EVENTS` in `constants.rs:134`
lists it under), one `ResizeObserver` observes the element (`x-view`,
`x-text`) or, for `x-overlay-ng`, its shadow root's first child. Each
observation dispatches a `CustomEvent('layoutchange')`, `bubbles: false,
composed: false, cancelable: true`, whose detail is
`{width, height, left, right, top, bottom, id}` from
`entry.target.getBoundingClientRect()` (transforms included); the
main-thread event path then subtracts the `<lynx-view>`'s own client origin
(`createCrossThreadEvent.ts:111-127`), so the numbers are lynx-view
relative. No `dataset`. The e2e tests `api-bindlauoutchange` and
`api-bindlayoutchange-lynx-view-relative` assert exactly those seven fields.

**What the two references disagree on**

| | native | web-core |
| --- | --- | --- |
| Position-only or edge-only change (a sibling grew; margin changed; size unchanged) | fires | does not fire (ResizeObserver is size-only) |
| Text re-measured to the same frame | fires (measure-func dirty) | does not fire |
| `dataset` in detail | present | absent |
| Transforms in the rect | included | included |
| First event | first frame after registration | ResizeObserver's initial observation |

**This engine.** Nothing: `docs/tracking/dom-events.md:61` lists it as
"Implement via ResizeObserver-equivalent"; `committed_box.rs` and
`css-layout.md:96` record "this engine has no ResizeObserver"; two
replication tests are ignored as GAP
(`web_text_replication.rs:1168`, `:1255`); `overlay.rs:176` notes
`x-overlay-ng` has no `layoutchange`. The engine's own `boundingClientRect`
UI method is a transform-free layout walk (`Document::bounding_client_rect`,
`layout/mod.rs:587`; the 2026-09 element-measurement ruling).

### 1.2 `layout` (`bindlayout`), `<text>` only

**Native.** `TextFragmentBehavior::DispatchLayoutEvent`
(`core/renderer/dom/fragment/text_fragment_behavior.cc:65-124`), fired
when the text was laid out and the element has a `layout` listener: once
per new text renderer on iOS (`LynxUIText.m:359-365`, the
`_didDispatchLayoutEvent` latch resets when the renderer object changes,
`:412-415`) and once per `TextUpdateBundle` on Android
(`TextHelper.java:602-616`). Detail:
`{lineCount, lines: [{start, end, ellipsisCount}], size: {width, height}}`,
the ranges in source units, the size the content box
(`TextLayoutEventDetail`, `events.d.ts:391`).

**web-core.** `XTextTruncation.ts:369-430`: dispatched from the truncation
pass (`#layoutText`, itself driven by the element's own `ResizeObserver`,
`:333`), with a lazy `Proxy` detail that measures `lineCount`/`lines` only
when read; no `size`. The upstream e2e test is permanently skipped
(`reactlynx.spec.ts:2944`, "the text layout event should be improved").

**This engine.** The data exists: `hughie::text::block::LineInfo`
(`crates/hughie/src/text/block/mod.rs:93-101`) carries `source_start`,
`source_end` and `ellipsis_count` per line, computed by every paragraph
layout. What is missing is the delivery and a host-visible query
(`Document::text_block` is `pub(crate)`).

### 1.3 Component events that carry a size, not element-generic

Listed for completeness; none is a candidate for the primitive below.

- `<list>` `layoutcomplete` (`core/renderer/ui_component/list/list_event_manager.cc`).
- `<scroll-view>` and `<x-swiper>` `contentsizechanged`
  (`UIScrollView.java:64`, `XSwiperUI.java:68`).
- `<x-foldview-ng>` `offset`.
- `uiappear`/`uidisappear` and the exposure records: built on the
  IntersectionObserver primitive (#463).
- iOS `LynxEventTypeLayoutEvent` (`LynxUIOwner.m:1062`,
  `LynxEventEmitter.mm:169`) is an internal "a layout finished" notice for
  the old polling intersection observer; it is not script-visible.

## 2. What the engine already has to build on

- **The IntersectionObserver primitive** (`crates/dom/src/visual/intersection/`):
  a `Vec` of observers in creation order, `Box<dyn Handler<T>>` per observer
  (user ruling 2026-10-10), `Registration {target, previous}` per target,
  a stale bit set by every built frame, moved scroll, observe and freed
  root, an update that reads the last layout with no pass, and a notify
  loop the host runs from a posted entry
  (`page.rs:987` `update_intersection_observations`, `:416`
  `notify_intersection_observers`). Node free tells the registry
  (`forget_node`). Zero-observer cost: one `is_empty` per freed node, one
  `bool` store per render, two tests per update.
- **The exposure adapter** (`crates/bobcat-core/src/main/exposure.rs`): a
  bucket-2 Lynx layer over that primitive, registered by the listener table
  (`open_listener`/`close_listener` gate on `is_exposure_event`,
  `runtime/lib.rs:1046-1096`; `bobcat:element` files the internal
  `bobcat:exposure` row), one observer per element because each element has
  its own thresholds and margins.
- **Per-node committed layout**: `LayoutSlot.rounded: Layout`
  (`hughie/src/tree/io.rs:268`: `location`, `size`, `content_size`,
  `border`, `padding`, `margin`), read in O(1) by `Document::rounded_layout`.
  The rounding tail already marks `BOX_CHANGED` per node whose unrounded box
  moved (`hughie/src/tree/mod.rs:96-101`, consumed and cleared in
  `compute/mod.rs:1938-1940`).
- **The committed-box table** (`committed_box.rs`): the content box of
  size-query containers and `contain-intrinsic-size: auto` boxes, recorded
  at the commit's own layout run because, as its doc says, there is no
  ResizeObserver to record it later. A ResizeObserver does not replace it:
  css-sizing-4 wants the recording at "the time ResizeObserver events are
  delivered", which the commit already is.
- **Component events** (`main/tree/lib.rs:320` `ComponentEvents`): a queue
  the epilogue posts as one entry (`page.rs:472`), used by `<image>` `load`,
  `<dialog>` `close`, instant-scroll `scroll`s.
- **The epilogue order** (`page.rs:975-1010`): instant scroll events,
  commit, text-input state, intersection update, posted deliveries. The
  commit is `DocumentSlot::commit_if_dirty` (`runtime/lib.rs:1507`), which
  calls `Document::commit()` (layout and frame build together) while the
  slot is borrowed; `after_timers` receives no `ScriptRuntime`.

## 3. The two paths

### 3.1 Path A: W3C ResizeObserver primitive + a Lynx `layoutchange` adapter

Mirror of the exposure design: a standards primitive in `dom`, a bucket-2
adapter in `bobcat-core`, registration driven by the listener table.

**Classification.** web-core's `layoutchange` *is* a `ResizeObserver` on the
element, so under the compat target (web-core) the event is bucket 1: a
real W3C feature with Lynx's event shape on top. Native's "own box changed"
is the deviation.

**The primitive** (`crates/dom/src/visual/resize/` or `layout/resize.rs`):

- `ResizeObservers<T>`: `Vec<ResizeObserver<T>>` in creation order, ids
  monotone, `Box<dyn ResizeEventHandler<T>>` per observer (the same `dyn`
  ruling as the IO primitive), a stale bit.
- `Registration { target: NodeId, box: ObservedBox, last_reported: Size<f32> }`,
  `last_reported` initialised to `(-1, -1)`, the current spec's initial
  `lastReportedSizes` (the CSSWG resolution on csswg-drafts issue 3664), so
  the first update reports every target once, a `0 × 0` or `display: none`
  one included.
- `ObservedBox`: `ContentBox` (default), `BorderBox`,
  `DevicePixelContentBox`. Horizontal writing mode only, so
  `inlineSize = width`, `blockSize = height`.
- `gather_active_resize_observations(depth)`: for each registration, read
  `rounded_layout(target)`; a node not rendered (no box, `display: none`) is
  `(0, 0)`; a target under a skipping `content-visibility` ancestor is left
  alone, neither compared nor updated, as Chromium leaves a target in a
  display-locked subtree; the observation is active when the box's size
  differs from `last_reported` and the target's depth (ancestor count) is
  greater than `depth`.
- `broadcast`: per observer with active observations, build the entries
  (`contentRect` = content size at `(padding.left, padding.top)`;
  `borderBoxSize`, `contentBoxSize`, `devicePixelContentBoxSize` = content
  size times the device pixel ratio, snapped), set `last_reported`, call the
  handler; return the shallowest depth delivered.
- Stale bit: set by a layout pass that ran (`Document::layout`), by
  `observe`, by a target's free. Not by scroll: scrolling resizes nothing.
- `forget_node`: drop registrations and queued entries naming the node;
  drop observers bound to it.
- The extras the user allows for Lynx are **not** in the primitive's entry.
  The adapter computes the viewport rect itself, for the resized elements
  only, so a standard observer pays nothing for it.

**The processing model** (spec §3.2, HTML "update the rendering"): resize
observations are gathered after layout and delivered **before paint**, in
a loop: gather at depth, broadcast, lay out again, gather at the shallowest
delivered depth, until nothing is active; whatever is still active but
shallower is a skipped observation, reported as the error
"ResizeObserver loop completed with undelivered notifications." That
differs from IntersectionObserver, whose spec says "queue a task", which is
why the IO delivery is a posted entry here. Two placements are possible:

1. **Spec placement, inside the commit.** The epilogue's commit becomes
   layout → resize loop (callbacks run in the realm, so the slot borrow is
   released around each broadcast; `after_timers` is handed the
   `ScriptRuntime` the way `run_due_timers` is) → frame build. A callback
   that resizes something costs one more incremental layout pass; the loop
   is bounded by tree depth and in practice ends after one or two passes.
   The frame then shows the callback's writes, as a browser's does.
2. **Posted entry, like IO.** One frame is built before the callback runs;
   its writes land in the next commit. Nothing changes in the epilogue's
   signature. Observable only by main-thread handlers that write styles in
   response (`main-thread:bindlayoutchange`): they land one frame late.
   Background-thread handlers see no difference.

Recommendation: placement 1, because the user asked for the browser
standard, and the seam it needs (layout separate from frame build) already
exists as `Document::layout` and `Document::commit`.

**The MTS surface.** A `ResizeObserver` global in the main-thread realm
(`bobcat-element`): `new ResizeObserver(callback)`, `observe(target,
{box})`, `unobserve`, `disconnect`, entries built in JavaScript from the
primitives the host passes per entry (`target` node, content `w h`,
padding-left/top, border-box `w h`, device-pixel `w h`), per the
JavaScript-data-ownership rule. Its handler queues `(observer, entries)` on
a runtime queue the host drains into the realm inside the loop (placement
1) or after it (placement 2), the same shape the IO doc reserves for its
MTS binding. web-core has no `ResizeObserver` in its MTS realm either (the
realm is a Worker there), so this is additive and can follow the adapter.

**The `layoutchange` adapter** (`crates/bobcat-core/src/main/layout_events.rs`):

- Registration: `open_listener`/`close_listener` gate on
  `name == "layoutchange"` as they gate on exposure names; on each change of
  `has_listener(node, "layoutchange")` the adapter observes or unobserves
  the node. Every registration form counts (`__AddEventListener`,
  `__AddEvent` `bindEvent`/`catchEvent`/capture/global-bind, string handler
  or worklet), which is web-core's `registerEventEnableStatusChangeHandler`.
  No attribute and no component code is involved; `x-overlay-ng` gets the
  element itself as target (web-core observes its shadow child because
  the host element has no box there; here the overlay element has one).
- One observer for the whole document, every registered element a
  `BorderBox` target. Per-element observers (the user's framing) are not
  needed: unlike exposure there are no per-element options, and the entry
  names its target. Cost difference: one `Box<dyn>` + two `Vec`s + ~200 B
  per element versus ~16 B per element.
- Delivery: the handler receives entries; for each, `bounding_client_rect(target)`
  (one containing-block walk, O(depth)) and a dispatch of `layoutchange` at
  the element, non-bubbling, not composed, cancelable, detail built in
  JavaScript from the node and four numbers: `{id, dataset, width, height,
  left, top, right, bottom}` (`id` and `dataset` read from the handle).
  Background-thread string handlers reach the BTS through the existing
  dispatch path with no new transport.
- No host-origin subtraction. web-core subtracts the `<lynx-view>`'s own
  client origin because its elements are rendered in the host page's
  layer and `getBoundingClientRect` answers in the host page's viewport.
  Here the document's viewport is the Lynx view itself
  (`Document::layout` lays out against `device().viewport_size()`), so
  `bounding_client_rect` is already Lynx-view relative, exactly what the
  `boundingClientRect` UI method answers.
- Semantics: fires on border-box size change only (web-core), once on the
  initial observation whatever the size (spec, `0 × 0` included), and with
  `(0, 0)` when the element stops being rendered.

### 3.2 Path B: a `LayoutObserver` of the element's own box

A Lynx-shaped primitive: `Registration { target, last: Layout }`,
active when `rounded_layout(target)` differs from `last` in location, size,
padding, border or margin, which is native's `SetNewLayoutResult` test.
Same registry shape, same stale bit, same delivery entry.

What it buys: native's position-only and edge-only events. What it costs:
a 72 B copy per registration instead of 8 B, a 72 B compare per
registration per commit instead of 8 B, and more events (every reflow that
shifts an observed element fires, which is also more cross-thread
traffic to the BTS). What it loses: it is not a standard anyone else
implements, and web-core, the compat target, does not fire those events,
so a card tested against web-core would see events it never saw before.
A text re-measured to the same box (native fires) would still not fire
unless the text engine reported "re-shaped" separately.

Path B can be expressed as Path A plus one more `ObservedBox` variant,
`OwnBox`, whose "size" is the whole `Layout`. That keeps one registry and
lets the semantic be a ruling rather than an architecture.

### 3.3 `<text>` `layout`: neither a ResizeObserver nor a RelayoutObserver

The event reports the paragraph algorithm's output (lines, truncation),
not a size: a `<text>` whose box stays 200×42 while its content changes
from one line to two still owes a `layout`, and a ResizeObserver cannot
see that. So the ResizeObserver is not its producer.

A generic **RelayoutObserver**, "tell me when this node's layout algorithm
re-ran", was considered and is not needed either. It would be a second
observer registry with one consumer, no W3C equivalent, and a hook in
hughie's cached dispatch for arbitrary nodes; and its payload would still
be text-engine data that only the text block can supply. What the event
needs is the precedent the engine already has for `content-visibility`:
**a per-pass report list collected by `dom` during the commit and taken by
the host in the epilogue** (`Document.content_visibility_changes`,
`relevance.rs:170-186`, `take_content_visibility_changes`).

Concretely:

- `compute_text_block_layout` (`layout/text_block.rs:482`) runs only when
  hughie's layout cache missed, and its `input.goal.commits()` branch is
  the committing run that recomputed the lines. There, when the element is
  flagged as reporting, push its id onto `Document.text_layout_reports`.
  The flag is membership in a node set on `DocumentLayoutState`
  (`text_layout_reporters`), added and removed by the host from
  `open_listener`/`close_listener` on the name `layout` (the same gate as
  `layoutchange`), so an unflagged paragraph pays one `is_empty` test per
  committing run and nothing else.
- The epilogue takes the list after the commit (deduplicated by node; a
  paragraph can commit twice in one pass under the container loop), reads
  each paragraph's `LineInfo`s through a new `pub` query on `Document`
  and the content box from `rounded_layout`, and queues one component
  event per node (`ComponentEvents`), delivered by the component-events
  entry (`post_component_events`), which is also where native delivers it
  (`runOnUiThread`). JavaScript builds `{lineCount, lines, size}` from the
  node, the size and a flat list of `(start, end, ellipsisCount)` triples.
- Firing on every committing re-layout of the paragraph, not only when the
  lines changed, is native's "once per new renderer". web-core's lazy
  `Proxy` is unnecessary because the lines are already computed.

## 4. Performance and memory

**With no registration on the page** (every page today):

| Where | Cost |
| --- | --- |
| per commit | one `is_empty` test on the resize registry (the IO primitive pays the same) |
| per `listenerOpened`/`listenerClosed` | one name compare, beside the exposure-name compare already there |
| per freed node | one `is_empty` test |
| per text paragraph layout | one `is_empty` test on the reporting set, only on a cache miss |
| memory | one empty `Vec` and a `bool` per document; `Node`, `LayoutSlot` and `NodeLayoutState` unchanged (the 224/336/352 assertion stands) |

**Per registered `layoutchange` element**, R registrations:

| Where | Cost |
| --- | --- |
| memory | ~24 B per registration in one `Vec` (document-wide observer: target, box kind, last reported size); ~200 B + 2 heap blocks each if one observer per element |
| per commit whose layout pass ran | O(R × depth): one walk up the target's box ancestors (the skip and display tests) and one 8 B compare each |
| per scroll-only frame | nothing (not stale), where the IO update must recompute geometry |
| per resized element | one O(depth) containing-block walk (~1 µs), one entry (~80 B), one JS `CustomEvent` dispatch, and for a BTS handler one structured clone of a ~100 B detail |
| resize loop (placement 1) | one extra incremental layout pass per depth level a callback resizes; zero when no callback resizes anything |
| worst case: viewport change with every element registered | N walks + N dispatches; the same order as the IO update over N targets; a per-parent memo of the walk is the lever if a census ever shows it |

**Per `<text>` `layout` registration**: one event per paragraph layout
carrying 3 × lines `u32` plus two `f32`; no per-commit scan.

**Path B** adds 64 B per registration and a 72 B compare per commit, and
multiplies events by the share of reflows that move observed elements
without resizing them.

## 5. Decisions (user rulings, 2026-10-20)

1. **Semantics of `layoutchange`**: web-core's size-only ResizeObserver
   semantics. An element that moves or changes an edge without changing its
   border-box size fires nothing. Native's own-box semantics (Path B /
   `OwnBox`) are not built.
2. **Delivery placement**: a posted entry after the frame, as the IO
   delivery is. The callback's writes land in the next commit; a
   main-thread handler that writes styles in response lands one frame
   later than in a browser. No change to the epilogue's signature. The
   spec's depth loop and loop error are therefore not built either: with
   delivery outside the commit, every observation is delivered at the next
   entry and none is skipped.
3. **Coordinates of the detail**: the engine's transform-free
   `Document::bounding_client_rect`, the same walk the `boundingClientRect`
   UI method answers from (its 2026-09-16 ruling). This is Android's
   default behavior; iOS and web-core include transforms, recorded as a
   deviation.
4. **`dataset` in the detail**: present, as native's is and as the
   engine's `boundingClientRect` UI method already answers:
   `{id, dataset, width, height, left, top, right, bottom}`. web-core
   omits it.
5. **Observer granularity**: one document-wide observer, every registered
   element a target.
6. **The MTS `ResizeObserver` global**: not ruled; default is the same
   series as the adapter, primitive first.
7. **`<text>` `layout`**: not a ResizeObserver and no RelayoutObserver
   primitive (user, 2026-10-20): a per-pass report list collected at the
   paragraph's committing run, delivered as a component event, with `size`
   included (§3.3).
8. **No host-origin subtraction** in the `layoutchange` rect (user,
   2026-10-20): the document viewport is the Lynx view (§3.1).

With rulings 1 and 2 the primitive keeps the spec's data model (observation
list, `lastReportedSizes`, box kinds, entry shape) and its gather step, and
delivers from a posted entry rather than inside "update the rendering".

**Built (2026-10-20).** The primitive is `crates/dom/src/visual/resize/`
and the text layout report list `Document::set_text_layout_reporting`,
`take_text_layout_reports` and `text_lines` (`crates/dom/src/layout/mod.rs`);
the host side is `crates/bobcat-core/src/main/layout_events.rs`, the resize
delivery entry `Page::post_resize_notifications` and
`MainThreadRuntime::{update_resize_observations, notify_resize_observers,
queue_text_layout_events}`, with the details built by `layoutDetailOf` in
`packages/bobcat-element/src/element-papi.ts`. Rulings 1 to 5, 7 and 8 are
built as written; the MTS `ResizeObserver` global (6) is not.
