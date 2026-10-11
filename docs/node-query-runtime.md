# Element PAPI and BTS node queries

This layer follows the native SelectorQuery facade used by ReactLynx. It runs
on the existing raw BTS environment, without requiring `lynx_core.js` or the
pending compiled-bundle initialization shell.

`bobcat:selector-query` owns `SelectorQuery` and `NodesRef` on BTS.
`lynx.createSelectorQuery()` sends operation, selection token, parameters and an
optional callback ID through `globalThis.postMessage`. MTS receives the request
through its existing Worker, calls `__BobcatQueryNodes` in `bobcat:element`, and
replies through `worker.postMessage`. The Worker's structured-clone transport
copies messages at send time. Rust transports Worker messages unchanged; only MTS host
members access the document. Named Lepus RPC keeps the asynchronous boundary
established in the preceding stack layer.

## Selection and callbacks

The native token contains `type`, `identifier`, `component_id`, `first_only` and
`root_unique_id`. Appending a task returns a new query; `exec()` replays the
queue without clearing it. Selection captures its root at select time.

- CSS selection uses the document's standard selector engine, including the
  Lynx query root. `selectRoot()` selects that root itself.
- `selectUniqueID()` is global once the supplied root exists.
- Legacy `selectReactRef()` selects by the `react-ref` attribute and runs the
  next task immediately; it is refused after queued work.
- `fields()` and `path()` call back with `(data, status)`. A successful status
  is `{code: 0, data: "success"}`. Missing nodes return code 2 with null for a
  single result or an empty array for multiple results; invalid selectors use
  code 5. A result value the transport refuses replies with code 1 and releases
  the callback.
- `invoke()` calls `success(result.data)` or `fail(result)`. Selecting all nodes
  fails locally with code 5. Missing targets use code 2. `boundingClientRect`
  exists on every element, a member of the base set the host answers after
  the element's kind, which never answers it
  (`crates/bobcat-core/src/main/tree/base_methods.rs:82`), and
  succeeds, with `{id, dataset, left, top, right, bottom, width, height}`: the
  border box in viewport CSS px as of the last completed layout pass, with
  ancestor scroll offsets applied and transforms ignored, `right` and `bottom`
  derived from the other four. `id` is the attribute, empty when the element
  carries none; `dataset` is the typed copy `__GetDataset` hands out, which
  native includes and web-core does not. `selectTab({index, smooth = true})`
  on a `viewpager` or `x-viewpager-ng` succeeds with no data as soon as the
  scroll is requested: the pager's element child number `index` (truncated
  toward zero) is scrolled into view with its start edge at the pager's
  inline start edge, the pager alone scrolling, smoothly unless `smooth` is
  falsy; a missing, non-numeric or negative `index`, or one naming no page,
  fails with code 4, `PARAM_INVALID`, and moves nothing.
  `scrollTo({index, smooth = true})` on an `x-swiper` succeeds the same way:
  the swiper's element child number `index` (truncated toward zero) is
  scrolled into view in its shadow `#content` at the item's snap position
  (its own `scroll-snap-align` on the main axis), with the same code 4 rules
  for `index`; Android's `direction` is ignored.
  `finishRefresh()`, `finishLoadMore()` and `autoStartRefresh()` on an
  `x-refresh-view` read no params and always succeed with no data: the first
  two smoothly scroll its shadow `#container` back to `#content`, the
  position the view opens on, and `autoStartRefresh` instantly scrolls the
  first header's start to the scrollport's start; a view with no header box
  records nothing. `autoStartRefresh` is instant where web-core's is
  smooth: the painter glides a smooth request to where the frame's snap
  rule would settle its target, and the header is a snap position only
  once it is at least 90% visible, so from rest a smooth request to it
  would be settled back to `#content`. A smooth version needs a request
  the painter does not settle at request time, a follow-up for the user.
  `setFoldExpanded({offset, smooth = true})` on a `scroll-coordinator` or
  `x-foldview-ng` succeeds with no data
  (`crates/bobcat-core/src/main/tree/scroll_coordinator.rs`): the
  coordinator, its own vertical scroll container, scrolls to `offset` (a
  number in px or a `px`/`rpx`/`ppx` string) clamped to its range, the
  header height less the toolbar height, smoothly unless `smooth` is falsy.
  A missing or unreadable `offset` fails with code 4 and moves nothing.
  iOS's `getScrollInfo` is code 3, and its `scrollBy({offset})` is not
  built: the name reaches the base set's CSSOM-View `scrollBy`, which reads
  no `offset`.
  `scrollTo({index?, offset = 0, smooth = false})`, `scrollBy({offset})` and
  `getScrollInfo()` on a `scroll-view` work on the axis it scrolls (x under
  `scroll-x` / `scroll-orientation="horizontal"`, else y): `scrollTo`
  succeeds with no data, scrolling to the `index`th element child plus
  `offset` (a number in px or a `px`/`rpx`/`ppx` string), or to `offset`
  alone, clamped to the range; `scrollBy` scrolls by a numeric `offset` at
  once and succeeds with `{consumedX, consumedY, unconsumedX, unconsumedY}`;
  `getScrollInfo` succeeds with `{scrollX, scrollY, scrollRange,
  maxScrollOffset}`, the last two both the axis's maximum offset, all in CSS
  px. Bad params, or an index naming no child, fail with code 4 and move
  nothing.
  `scrollToPosition({position, index?, itemKey?, offset = 0, alignTo =
  "top", smooth = false})`, `scrollBy({offset})`, `getScrollContainerInfo()`
  and `getVisibleCells()` on a `list` work on the axis it scrolls (x under
  `scroll-orientation="horizontal"`, else y)
  (`crates/bobcat-core/src/main/tree/list.rs`). `scrollToPosition` succeeds
  with no data: the cell is the list's element child whose `item-key`
  equals a string `itemKey`, else element child number `index`, else
  number `position` (truncated toward zero; `index` is read first), and
  the target is its start in the list's scroll coordinates, less half of
  what the scrollport has beyond the cell for `alignTo: "middle"` or all of
  it for `"bottom"` (anything else is `"top"`), plus `offset` (read as
  `scroll-view`'s `scrollTo` reads it), clamped to the range; smoothly when
  `smooth` is truthy. A missing, non-numeric, negative or out-of-range
  number, or a bad `offset`, fails with code 4 and moves nothing; a list or
  cell with no box succeeds and moves nothing. `scrollBy` is
  `scroll-view`'s. `getScrollContainerInfo` succeeds with web-core's
  `{scrollTop, scrollLeft, scrollHeight, scrollWidth}`, the offset and the
  scrolling area. `getVisibleCells` succeeds with an array of `{id, itemKey,
  index, left, top, right, bottom}` for each `list-item` child whose border
  box overlaps the scrollport, `id` and `itemKey` the attributes (empty when
  absent), `index` the position among the list's `list-item` children, the
  rect in CSS px relative to the list's scrollport (native's coordinates;
  web-core reports the viewport's). `autoScroll` and `getScrollInfo` are
  code 3, and a `list` has no `scrollTo` of its own, so the base set's
  CSSOM-View `scrollTo` answers one.
  `show`, `showModal`, `close` and `requestClose` on a `dialog` are HTML's
  `HTMLDialogElement` methods and succeed with no data; HTML's
  `InvalidStateError` (`show` on a modal dialog, `showModal` on an open
  non-modal or a disconnected one) fails with code 7, `INVALID_STATE_ERROR`
  (native's code; web-core answers 4 for any method that throws, a ruled
  deviation recorded in `docs/tracking/deviations.md`), and changes nothing;
  `params` is not read.
  `scrollIntoView({scrollIntoViewOptions: {behavior?, block?, inline?}})`
  exists on every element and succeeds with no data once the scroll is
  requested: it is CSSOM-View's `scrollIntoView` with `container: "nearest"`
  (`Document::scroll_into_view`, `crates/dom/src/scroll/into_view.rs:188`,
  run by `crates/bobcat-core/src/main/tree/base_methods.rs:252`), so only the
  nearest scroll container above the element moves, to the position
  `block` (default `"start"`) and `inline` (default `"nearest"`) choose
  against its `scroll-padding` and the element's `scroll-margin`, smoothly
  when `behavior` is `"smooth"`. A missing `scrollIntoViewOptions` object
  fails with code 4; an element with no box or no scroll container above it
  fails with code 8, `OPERATION_ERROR`, native's answer (web-core answers 0,
  recorded in `docs/tracking/deviations.md`).
  `scrollTo({left?, top?, behavior?})` and `scrollBy({left?, top?,
  behavior?})` also exist on every element whose kind has no method of the
  name, and succeed with no data: they are CSSOM-View's
  `Element.scrollTo`/`Element.scrollBy` with a `ScrollToOptions` dictionary
  (`crates/bobcat-core/src/main/tree/base_methods.rs:143`). `left` and `top`
  are CSS px, a value that is no finite number after Web IDL's conversion
  reading as 0; a missing one keeps that axis for `scrollTo` and adds 0 for
  `scrollBy`; the target clamps to the range, smoothly only for
  `behavior: "smooth"` (`"auto"` and `"instant"` are instant: there is no
  `scroll-behavior`). A `behavior` that is present and not one of those
  three strings, or params that are a number, a string or a boolean, fail
  with code 4 and move nothing, as web-core answers the `TypeError` Web
  IDL throws for them; `null` and an array read as `{}`. On an element with
  no box or that is no scroll container they do nothing and succeed, as
  web-core's do. A `scroll-view`'s and an `x-swiper`'s own `scrollTo`,
  and a `scroll-view`'s own `scrollBy`, answer instead, and read their Lynx
  params, not these. `focus`, `blur` and `takeScreenshot` are code 3: there
  is no focus model and no pixel readback.
  Every other method name, and each kind's method on any other element,
  fails with code 3, `METHOD_NOT_FOUND` — web-core's code, where native answers its
  generic 1. A failure without a fail callback is ignored by the production
  facade. `params` crosses to the main thread as JSON text.
- The measurement runs no flush. It reports the last completed pass, so a BTS
  query is always current: the entry that queued it has already returned and
  its epilogue committed, and `setNativeProps` commits before the next request
  in the same queue. An MTS worklet that mutates and measures inside one job
  sees the pre-mutation geometry until it calls `__FlushElementTree`, which is
  also the order ReactLynx's own `Element.invoke` uses.
- BTS owns callbacks and removes each after delivery, including callbacks that
  throw. Query callbacks run in the message handler; they are distinct from the
  Promise continuation used by named Lepus RPC.

## Values and mutations

Fields support `id`, `tag`, `unique_id`, `name`, `index`, `class`, `attribute`,
`dataset`/`dataSet`, and `query`. For `query: true`, BTS requests `unique_id` and
constructs a rooted query in the response callback. `path()` returns field
records from the selected node through its page ancestors.

`__SetDataset` merges typed keys; `__AddDataset` replaces one key. Assignment
and `__GetDataset` copy containers. Datasets stay separate from DOM `data-*`
strings. Background event descriptors include those typed dataset values.
Lynx attributes similarly retain typed copies for readback while ordinary DOM
attributes remain strings. Attribute fields exclude function/null/undefined
values and the separate id/class/style/dataset stores. Assigning a local
attribute does not impose the Worker transport's own restrictions.

`__AddInlineStyle` changes one named CSS property without replacing the rest
of the declaration block. Empty/nullish values remove it; the existing CSSOM
path owns parsing and invalid-value handling. `setNativeProps` classifies names
through Stylo, updates CSS properties or attributes, then commits before the
next queued query. Inputs use CSS strings; numeric native CSS property IDs and
native numeric-length conversion remain unsupported.

## Remaining boundaries

Legacy component-scoped roots, direct MTS selector PAPI, UI invoke methods
other than the base set's four (`boundingClientRect`, `scrollIntoView`,
`scrollTo`, `scrollBy`), `selectTab`, a swiper's `scrollTo`, a `scroll-view`'s three, a `list`'s four, an `x-refresh-view`'s three, a scroll coordinator's `setFoldExpanded` and a dialog's four (a `list`'s `autoScroll` and a coordinator's `getScrollInfo` among them), animation methods,
dataset-to-DOM reflection and cross-realm host objects remain pending.
An `image`'s `startAnimate`/`pauseAnimation`/`resumeAnimation`/`stopAnimation`
and a `text`'s `getTextBoundingRect`/`getSelectedText`/`setTextSelection`
are code 3 for want of a primitive: the engine plays no image frames, and has
no text selection model and no query from a character range to rects.
`boundingClientRect` takes no `relativeTo`, `androidEnableTransformProps` or
`iOSEnableAnimationProps`: it reads nothing from its `params`, since each of
those names behavior this engine does not have. Query failures and invalid legacy ReactRef chaining
now use the nonfatal [runtime reporter](events-diagnostics-runtime.md), alongside
the existing query status replies.
Compiled ReactLynx bundle loading and ref hydration integration remain later
stack layers; these tests exercise raw BTS with real QuickJS and document nodes.

## Native evidence and verification

The reference checkout is `lynx/` as located in `AGENTS.md`:

- `js_libraries/lynx-core/src/modules/selectorQuery/SelectorQuery.ts`: task
  branching/replay, selection tokens, rooted queries and invoke callbacks.
- `core/renderer/dom/fiber/fiber_node_info.cc`: fields, typed datasets and
  attribute filtering, plus path-to-root order.
- `core/renderer/dom/attribute_holder.cc`: dataset merge behavior.
- `core/renderer/page_proxy.cc`: structural result/status shapes and selection.

Rstest checks the facade, container copies, refused result values and callback
cleanup.
Core tests run both real QuickJS realms, query the actual DOM, and verify that
native props are applied before a later query observes them. Inline-style
coverage checks the resulting declaration block and layout through Stylo.
