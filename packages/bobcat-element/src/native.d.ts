/**
 * The native functions exported by `bobcat-internal:host`. They speak DOM
 * vocabulary over numeric `NodeId`s and own the document and style/layout
 * commit; misuse crashes at this boundary instead of being validated.
 */
interface BobcatNative {
  /**
   * Builds the realm's one document out of the five page switches, in
   * `PageConfig`'s order, and the view's own resources, which never reach
   * this realm: its metrics, its fonts and its style pool.
   *
   * It never waits. The view's author stylesheets are not mounted here: the
   * first `flushElementTree` mounts them, in the order the view listed them.
   * A switch that is not a boolean, and a realm that already has a document,
   * both throw.
   *
   * Returns the `NodeId` of the document node, the target of the document's
   * own events (`selectionchange`).
   */
  createDocument(
    defaultDisplayLinear: boolean,
    defaultOverflowVisible: boolean,
    enableCssSelector: boolean,
    enableJSDataProcessor: boolean,
    enableCssInheritance: boolean,
  ): number;
  /**
   * Returns the `NodeId` of the document's permanent `page`, in every realm.
   * A frame's `ShadowRealm` child does not call it: its `__CreatePage`
   * answers the nested page `bobcat:frame-boot` named.
   */
  createPage(): number;
  /** Creates a detached element and returns its `NodeId`. */
  createElement(tag: string): number;
  setAttribute(nodeId: number, name: string, value: string): void;
  /**
   * Replaces the whole inline declaration block from one record payload: a
   * flat sequence of `<utf16Length>:<text>` fields, name then value, two per
   * declaration. An empty payload leaves an empty `style` attribute.
   */
  setInlineStyles(nodeId: number, record: string): void;
  setInlineStyleProperty(nodeId: number, name: string, value: string): void;
  supportsStyleProperty(name: string): boolean;
  /**
   * The `NodeId`s matching `selector` under `root`, joined by commas, or the
   * empty string for no match. `includeRoot` picks the scope: `1` is Lynx's
   * SelectorQuery, which considers `root` itself first, `0` is
   * `Element.querySelector`'s, which does not.
   */
  queryElementIds(
    root: number,
    selector: string,
    firstOnly: 0 | 1,
    includeRoot: 0 | 1,
  ): string;
  removeAttribute(nodeId: number, name: string): void;
  /** The attribute's value, or null when the element does not carry it. */
  getAttribute(nodeId: number, name: string): string | null;
  /** The element's local name, verbatim as it was created. */
  tagName(nodeId: number): string;
  /**
   * Every attribute name the element carries, in acquisition order, as one
   * record payload: a flat sequence of `<utf16Length>:<text>` fields, one per
   * name. Empty when the element carries none.
   */
  attributeNames(nodeId: number): string;
  /**
   * Dispatches one Lynx UI method by name on the element, with `params` as
   * JSON text — the empty string for `boundingClientRect`, which reads
   * none — and answers either a status code or the method's data as a
   * record. Every name is asked of the element's kind first (its
   * component's `terminus::link::CustomElement::invoke`) and, when the kind has no
   * method of that name, of the base set every element has
   * (`boundingClientRect`, `scrollIntoView`, `scrollTo`, `scrollBy`); the
   * host matches no name itself:
   *
   * - `3` (`METHOD_NOT_FOUND`): neither the element's kind nor the base set
   *   has a method of that name. `focus`, `blur` and `takeScreenshot` are
   *   among them.
   * - `4` (`PARAM_INVALID`): the method refused `params` and did nothing.
   * - `7` (`INVALID_STATE_ERROR`): the element's state refused the method
   *   (a dialog's `InvalidStateError`), and it did nothing.
   * - `8` (`OPERATION_ERROR`): the method ran and its operation failed.
   * - `0` (`SUCCESS`): the method ran and has no data to answer.
   * - a string: a record (`bobcat:record`) whose first field is the status
   *   code `0` and whose remaining fields are the method's data, as the
   *   method wrote them. Six methods answer one: `boundingClientRect`,
   *   whose fields are `left`, `top`, `width`, `height` in that order, the
   *   border box in viewport CSS px as of the last completed layout pass,
   *   ancestor scroll offsets applied, transforms ignored (native's own
   *   conversion ignores them too), zeros for an element with no box; a
   *   `scroll-view`'s and a `list`'s `scrollBy`, whose fields are
   *   `consumedX`, `consumedY`, `unconsumedX`, `unconsumedY` in that order;
   *   a `scroll-view`'s `getScrollInfo`, whose fields are `scrollX`,
   *   `scrollY`, `scrollRange`, `maxScrollOffset` in that order; a `list`'s
   *   `getScrollContainerInfo`, whose fields are `scrollTop`, `scrollLeft`,
   *   `scrollHeight`, `scrollWidth` in that order; a `list`'s
   *   `getVisibleCells`, whose first field is the number of cells, followed
   *   by seven fields per cell: `id`, `itemKey` (both strings, empty when
   *   the cell carries no such attribute), `index`, `left`, `top`, `right`,
   *   `bottom`; and a `text`'s `getTextBoundingRect`, whose first field is
   *   the number of boxes, followed by `left`, `top`, `width`, `height` per
   *   box. Every numeric field is CSS px but `index` and the counts.
   *
   * `selectTab({index, smooth = true})` exists on `viewpager` and
   * `x-viewpager-ng` only: it scrolls the pager's element child number
   * `index` (truncated) into view at the pager's inline start edge,
   * smoothly unless `smooth` is falsy; an `index` that is not a number, is
   * negative, or names no page is code 4 and moves nothing.
   *
   * `scrollTo({index, smooth = true})` on `x-swiper` is the swiper's own
   * overload: it scrolls the swiper's element child number `index`
   * (truncated) into view in its shadow `#content`, at the item's snap
   * position, smoothly unless `smooth` is falsy; an `index` that is not a
   * number, is negative, or names no item is code 4 and moves nothing.
   * Android's `direction` is not read.
   *
   * `scrollTo({index?, offset = 0, smooth = false})`, `scrollBy({offset})`
   * and `getScrollInfo()` exist on `scroll-view` only, on the axis its
   * `scroll-x` / `scroll-orientation` attributes scroll. `scrollTo` scrolls
   * to the `index`th element child's position plus `offset` (a number in
   * px, or a `px`/`rpx`/`ppx` string), or to `offset` alone without an
   * `index`, clamped to the range, smoothly when `smooth` is truthy; an
   * index naming no child is code 4. `scrollBy` scrolls by a numeric
   * `offset` at once and answers what it consumed; `getScrollInfo` answers
   * the offset and the axis's maximum offset.
   *
   * `autoScroll({rate, start = false, autoStop = true})` exists on
   * `scroll-view` and `list`, on the axis the tag scrolls. With a truthy
   * `start` it moves that axis at `rate` CSS px per second (a number, or a
   * `px`/`rpx`/`ppx` string; negative toward the start edge) until the edge
   * it moves toward, where it ends unless `autoStop` is `false`, in which
   * case it goes on when the content grows; a zero, missing or unreadable
   * `rate` is code 4 and changes nothing. Without `start` it ends the
   * auto-scroll where it stands. It yields to a finger and to every other
   * scroll and goes on afterwards; `scrollTo` and `scrollBy` do not end it.
   * Always `0` and no data otherwise.
   *
   * `scrollToPosition({position, index?, itemKey?, offset = 0, alignTo =
   * "top", smooth = false})`, `scrollBy({offset})`,
   * `getScrollContainerInfo()` and `getVisibleCells()` exist on `list`
   * only, on the axis its `scroll-orientation` attribute scrolls.
   * `scrollToPosition` scrolls to the list's element child named by a
   * string `itemKey` that equals a child's `item-key`, else by `index`, else
   * by `position` (truncated), aligned by `alignTo` (`"middle"`,
   * `"bottom"`, anything else `"top"`), plus `offset` (read as
   * `scroll-view`'s `scrollTo` reads it), clamped to the range, smoothly
   * when `smooth` is truthy; a missing, non-numeric, negative or
   * out-of-range number is code 4. `scrollBy` is `scroll-view`'s.
   * `getVisibleCells` reports the `list-item` children whose border box
   * overlaps the scrollport, `index` counting `list-item` children, the
   * rect relative to the scrollport. `autoScroll` is `scroll-view`'s and
   * `getScrollInfo` is code 3. A `list` has no `scrollTo` of its own: the
   * base set's answers.
   *
   * `finishRefresh()`, `finishLoadMore()` and `autoStartRefresh()` exist on
   * `x-refresh-view` only, `params` unread, and always answer `0` and no
   * data. The first two smoothly scroll the view's shadow `#container` so
   * its `#content` is back at the scrollport's center, the position the
   * view opens on; `autoStartRefresh` instantly scrolls it so the first
   * header's start is at the scrollport's start, and records nothing when
   * no header has a box. `autoStartRefresh` is instant where web-core's is
   * smooth: the painter settles a smooth request's target onto the frame's
   * snap positions when it receives it, and the header is a snap position
   * only once it is at least 90% visible, so from rest a smooth request to
   * it would be settled back to `#content`.
   *
   * `setFoldExpanded({offset, smooth = true})` exists on
   * `scroll-coordinator` and `x-foldview-ng` only: it scrolls the
   * coordinator to `offset` (a number in px, or a `px`/`rpx`/`ppx` string)
   * clamped to its range, the header height less the toolbar height,
   * smoothly unless `smooth` is falsy, and answers `0` and no data; a
   * missing or unreadable `offset` is code 4. iOS's `getScrollInfo` is
   * code 3; its `scrollBy({offset})` is not built, so the name reaches the
   * base set's `scrollBy`, which reads no `offset`.
   *
   * `scrollIntoView({scrollIntoViewOptions: {behavior?, block?, inline?}})`
   * exists on every element: CSSOM-View's `scrollIntoView` on the nearest
   * scroll container above the element alone (`container: "nearest"`),
   * smoothly when `behavior` is `"smooth"`, with `block` (default `"start"`)
   * and `inline` (default `"nearest"`) each one of `"start"`, `"center"`,
   * `"end"` and `"nearest"`, any other value reading as `"start"`. A
   * missing `scrollIntoViewOptions` object is code 4; an element with no
   * box or no scroll container above it is code 8. It answers no data.
   *
   * `scrollTo({left?, top?, behavior?})` and `scrollBy({left?, top?,
   * behavior?})` exist on every element whose kind has no method of the
   * name: CSSOM-View's `Element.scrollTo`/`Element.scrollBy` with a
   * `ScrollToOptions` dictionary. `left` and `top` are CSS px, a value that
   * is not a finite number after Web IDL's conversion reading as 0; a
   * missing one keeps that axis for `scrollTo` and adds 0 for `scrollBy`.
   * The target clamps to the scroll range; `behavior: "smooth"` is smooth,
   * and `"auto"` and `"instant"` are instant, since there is no
   * `scroll-behavior`. A `behavior` that is present and not one of those
   * three strings, or params that are a JSON number, string or boolean, is
   * code 4 and nothing moves (web-core's answer to Web IDL's `TypeError`);
   * `null` and an array read as `{}`. An element with no box or that is no
   * scroll container does nothing. Otherwise both answer `0` and no data. A `scroll-view`'s and an `x-swiper`'s own `scrollTo`, and a
   * `scroll-view`'s own `scrollBy`, are asked first and answer instead.
   *
   * `getTextBoundingRect({start, end})` exists on `text` only: the boxes
   * the source range `[start, end)` of the paragraph the element
   * establishes occupies, in UTF-16 units of its text (an inline image or
   * view counting one), one box per line the range shows content on, from
   * the first selected glyph's left edge to the last one's right edge with
   * the line's top and height, in viewport CSS px (the space
   * `boundingClientRect` answers in; `relativeTo` is not read). `start`
   * and `end` must be numbers, are truncated toward zero, and must then be
   * non-negative with `start <= end`, else code 4. `start == end` answers
   * one zero-width box at the content box's origin, as tall as the line
   * holding `start`. A range past the text's end, a range that shows no
   * content (text past a truncation cut, the whitespace a soft wrap
   * consumed), a nested `text` (a run of its ancestor's paragraph) and an
   * element with no box are code 8. `getSelectedText` and
   * `setTextSelection` are code 3: there is no text selection model.
   *
   * `show`, `showModal`, `close` and `requestClose` exist on `dialog` only:
   * HTML's `HTMLDialogElement` methods, `params` unread. `close` and
   * `requestClose` queue their events for an entry of their own.
   *
   * Runs no style, layout or paint: it reads the last completed pass, and
   * user code decides when to flush.
   */
  callElementMethod(nodeId: number, method: string, params: string): string | number;
  /**
   * The element's computed style as one record payload: a flat sequence of
   * `<utf16Length>:<text>` fields, name then value, two per property.
   *
   * The whole style is every author-facing longhand followed by every custom
   * property present, each group sorted by code point; a non-empty
   * `properties` — a comma-separated name list — asks for those names only,
   * in which case an unknown or shorthand name is simply absent from the
   * answer rather than an error.
   *
   * `resolved` `1` substitutes CSSOM resolved values — the used px of the
   * last layout pass — for `width`, `height`, `margin-*` and `padding-*`
   * when the element has a box; `0` reports computed values throughout.
   * Empty before the first flush. Runs no flush.
   */
  getComputedStyleMap(
    nodeId: number,
    properties: string,
    resolved: 0 | 1,
  ): string;
  /**
   * The `NodeId`s of the element's element children, in tree order, joined by
   * commas — no length prefix, because a decimal id cannot contain the
   * separator. Empty when the element has no element children. Child *nodes*
   * that are not elements, such as the text node a `raw-text` reflects, are
   * not in it.
   */
  childElementIds(nodeId: number): string;
  /** The parent's `NodeId`, or null for a detached element. */
  parentNode(nodeId: number): number | null;
  /**
   * The node's position in its parent's child list, nodes of every kind
   * counted, or -1 for a detached element. Kept on the node, so O(1).
   */
  childIndex(nodeId: number): number;
  /**
   * Lets `nodeId`, which no commit has determined yet, start out as relevant
   * to the user as `from` was last found, and queues the
   * `contentvisibilityautostatechange` that first determination owes. For a
   * `content-visibility: auto` box built to take another's place: it is laid
   * out at its real size, not its `contain-intrinsic-size`.
   */
  inheritRelevance(nodeId: number, from: number): void;
  /** Reparenting insert; appends when `reference` is null. */
  insertBefore(parent: number, child: number, reference: number | null): void;
  /**
   * Detaches `child` from its parent; a no-op when already detached. Frees
   * nothing: the handle that names the child still holds it.
   */
  removeElement(child: number): void;
  /**
   * Replaces `oldElement` in place, leaving it detached — and live, held by
   * its handle.
   */
  replaceElement(newElement: number, oldElement: number): void;
  /** Exchanges two distinct attached elements, in or across parents. */
  swapElement(childA: number, childB: number): void;
  /**
   * Frees the element of a collected handle, and only it: its element
   * children are unlinked into detached roots, and what no handle could name
   * — the text node a `raw-text` reflects — goes with it. A `frame` takes
   * its shadow tree with it, as every element does. In a child that called
   * `closeRealm` it does nothing and answers `undefined`.
   */
  dropElement(nodeId: number): void;
  /**
   * Commits pending mutations through style and layout.
   *
   * The first call waits for every author stylesheet the view listed and
   * mounts them in listed order before anything is styled; a sheet that
   * failed to load throws `loading stylesheet <url>: <reason>`. Before a
   * painter has bound the view it then waits for the binding. The same in
   * every realm, a frame's `ShadowRealm` child included.
   *
   * `pipelineId` is the `pipelineOptions.pipelineID` the flush was called
   * with, as a number (`0` for none), and `timingFlag` its
   * `__lynx_timing_flag` option (`""` for none). A flush that names a
   * pipeline carrying timestamps, carries a flag, follows a
   * `__lynx_timing_flag` attribute, a native pipeline or the realm's first
   * render records the commit's timing and binds those pipelines to the
   * frame it committed; any other flush reads one flag and records nothing.
   */
  flushElementTree(pipelineId: number, timingFlag: string): void;
  /**
   * Records that this realm listens for `eventName` on the element in the
   * passes `kinds` names, replacing what it reported there before: the or of
   * 1 (the bubble pass: `__AddEventListener`'s bubble list, `bindEvent`,
   * `catchEvent`), 2 (the capture pass: the capture list, `capture-bind`,
   * `capture-catch`) and 4 (`global-bindEvent`), at least one. Called on a
   * change of that union alone. The host keeps it in the document's listener
   * table, which decides which realms a dispatch calls, routes the painting
   * side's name set, and detects an element with any registration for
   * `uiappear` or `uidisappear` for exposure. A node that is no longer live
   * registers nothing.
   */
  listenerOpened(nodeId: number, eventName: string, kinds: number): void;
  /**
   * The reverse: the realm's last registration for `eventName` on the
   * element went away. A collected element's rows leave with it, so a
   * collection calls nothing.
   */
  listenerClosed(nodeId: number, eventName: string): void;
  /**
   * The newest reading of the view's timeline the host was handed — the
   * last frame's or vsync's — in milliseconds since the view was built, `0`
   * before the first frame: the clock every event the host dispatches is
   * stamped on. `__CreateEvent` reads it once, as it builds an event.
   */
  timelineMilliseconds(): number;
  /**
   * The host's half of `__DispatchEvent`: the dispatch the kernel runs for a
   * routed event, the same code, for an event named `eventName` (lowercased,
   * as the realm files every name) at the live element `nodeId`. It computes
   * the path from `bubbles` and `composed`, runs the engine components' walk
   * over it with an empty detail and `timestamp`, and at the path's top step
   * calls `__BobcatDispatchEvent` once in each realm the listener table
   * invites (a registration for the name on the path, or a
   * `global-bindEvent` the path reaches), the page realm first and then the
   * frames' cards in construction order, before the components on the steps
   * below it. Every realm but this one is handed `detail` — the event's own,
   * structured-cloned at this call, so a value the clone refuses throws here
   * — as a `DETAIL_CLONED` detail, or a `DETAIL_EMPTY` one for `undefined`
   * or `null`; this realm, when it is invited, is handed `DETAIL_DISPATCHED`
   * and walks the event `__DispatchEvent` left pending. No call checkpoints,
   * since this runs inside a JavaScript call. What any realm's listener
   * throws is reported as that realm's listener failure and never reaches
   * this call. A listener may call it again from inside a walk. A node that
   * is not a live element, and a timestamp that is not a finite number,
   * throw.
   */
  dispatchEvent(
    nodeId: number,
    eventName: string,
    bubbles: boolean,
    composed: boolean,
    timestamp: number,
    detail: unknown,
  ): undefined;
  /**
   * `lynx.stopExposure({sendEvent})` (`on` `0`) and `lynx.resumeExposure()`
   * (`on` `1`), as native runs them for one card: for the elements `ids`
   * names — comma-joined node ids, as `childElementIds` answers a list, `""`
   * for none — which are the ones the calling realm registered: only it
   * knows which elements are its own. The host keeps one stopped mark per
   * element. Stopping marks each listed element stopped, and its detection
   * stops; with `sendEvent` `1` each of them that is exposed is sent one
   * `disexposure` record and forgotten as exposed, with `0` nothing is sent
   * and the exposed record is kept. Resuming clears the mark of each listed
   * stopped element and detects it again, and one that is visible and not
   * recorded as exposed is exposed again; a listed element that is running
   * is left alone. An element no switch lists is untouched, and an id that
   * names no registered element is skipped. `sendEvent` is read by a stop
   * alone.
   *
   * Answers how many records it queued, which the next intersection
   * delivery entry dispatches at their elements as `bobcat:exposure`
   * events: one per listed element exposed under an `exposure-id`, for a
   * stop with `sendEvent` `1`; `0` for anything else.
   */
  switchExposure(on: 0 | 1, sendEvent: 0 | 1, ids: string): number;
  /**
   * Arms one timer `delayMilliseconds` from now — repeating until cleared
   * when `repeats` — and returns the id it is armed under. The delay goes
   * through HTML's `long` conversion and nesting clamp here, so any number
   * is accepted.
   */
  setTimer(delayMilliseconds: number, repeats: boolean): number;
  /** Disarms a timer, whether or not one is armed under that id. */
  clearTimer(id: number): void;
  /**
   * The init data the view was given, as the string the host passed in —
   * unread and unparsed — or `undefined` when it was given none. Answers
   * once: the string is handed over, not kept.
   */
  initData(): string | undefined;
  /** The view's global props, handed over like `initData`. */
  globalProps(): string | undefined;
  /**
   * The native modules the embedder injected when the view was built, as one
   * record payload: a flat sequence of `<utf16Length>:<text>` fields, two per
   * module — its `NativeModules` key, then its method names joined with
   * commas, empty for a module that declared none. Empty for a view built with
   * no modules at all. This realm has no `NativeModules` and posts the record
   * unread to its BTS Worker in `initialize`. Answers once, like `initData`,
   * and `undefined` after that and in a `ShadowRealm` child, which is handed
   * the page realm's table instead.
   */
  nativeModuleTable(): string | undefined;
}

/**
 * The native functions a worker realm gets instead of the document, under
 * `bobcat-internal:worker`. Its `bobcat-internal:host` carries only the
 * members every realm is opened with — `requestScriptFrame`, the timer pair,
 * the `Future` members, `fetchResource`, `resolveUrl`, `resolveModuleUrl`,
 * `loadModuleSync`, `reportScriptError` and `logScriptMessage` — and none of
 * the document members, because a worker has no tree to mutate.
 */
interface BobcatWorkerNative {
  /**
   * Hands one message back to the realm that created us. Any value the host
   * boundary carries: a primitive, or a structured clone of anything else.
   * A value the engine's serializer refuses throws at this call.
   */
  postWorkerMessage(data: unknown): void;
  /**
   * Ends this worker once the running task returns. Queued messages and armed
   * timers go with it.
   */
  closeWorker(): void;
  /**
   * The worker's `self.name`, as its constructor named it: empty when it
   * named none. `bobcat:worker` reads it in its last statement. Answers once:
   * the string is handed over, not kept. The host takes the read as the
   * whole of `bobcat:worker` having run in this realm, and delivers a posted
   * message only to a realm in which it has.
   */
  workerName(): string;
  /**
   * The runtime's shared object: one null-prototype object, the same
   * reference in every worker realm of the group, so what one realm stores
   * on it another reads as itself. It lives while any of those realms does.
   */
  sharedObject: Record<PropertyKey, unknown>;
}

declare module "bobcat-internal:host" {
  /** Initial processor name, handed over once as a plain string. */
  export function initialProcessor(): string | undefined;
  export function requestScriptFrame(pending: boolean): void;
  /**
   * Whether the view's fetcher holds a stylesheet at `url` (as the realm
   * names it, unresolved), answered synchronously from what is registered:
   * no request, no wait, no network. `true` for a fetcher that cannot tell.
   * `__LoadStyleSheet` answers `null` when this is `false`.
   */
  export function hasStyleSheet(url: string): boolean;
  export function preloadStyleSheet(url: string): void;
  /**
   * Requests the stylesheet at `url`, waits for it inside the job and mounts
   * it: on the shadow root `shadowRoot` names (a frame's, which
   * `bobcat:frame-boot` mounts its card's page CSS on), or on the document
   * without one, behind the view's listed sheets. Throws for a
   * `shadowRoot` that is no live shadow root, and for a sheet that fails.
   */
  export function adoptStyleSheet(url: string, shadowRoot?: number): void;
  /**
   * Reports one diagnostic to the embedder as `EngineEvent::ScriptReported`,
   * named by this realm. `bobcat:diagnostics` passes `"warn"`, `"error"` or
   * `"fatal"` as `level`. The host sends it itself and relays it through no
   * other realm; nothing is thrown and nothing ends.
   */
  export function reportScriptError(level: string, message: string): void;
  /**
   * The same for console output, as `EngineEvent::ConsoleMessage`: `level` is
   * the console method's name.
   */
  export function logScriptMessage(level: string, message: string): void;
  /**
   * Starts one worker over the script `url` names and answers the key its
   * other members name it by.
   *
   * The host joins `url` to `baseUrl` — the realm's `__Card__`: the view's
   * card URL (a container page's container URL), or else the page entry's
   * response URL; in a frame's realm, the frame's card — by URL rules, and
   * does not keep the base. A `url` that does not resolve starts nothing and
   * answers `null`.
   *
   * A `ShadowRealm` child of this realm has these three members too, over a
   * side of its own: what its workers say is delivered to the child, a
   * child is not freed while one of its workers runs (unless it closed
   * itself with `closeRealm`), and freeing it ends the workers it has left.
   */
  export function createWorker(
    url: string,
    name: string,
    baseUrl: string,
  ): number | null;
  /**
   * Posts one message to that worker. Any value the host boundary carries; a
   * value the engine's serializer refuses throws at this call.
   */
  export function sendWorkerMessage(key: number, data: unknown): void;
  export function terminateWorker(key: number): void;
  export const createDocument: BobcatNative["createDocument"];
  export const createPage: BobcatNative["createPage"];
  export const createElement: BobcatNative["createElement"];
  export const setAttribute: BobcatNative["setAttribute"];
  export const setInlineStyles: BobcatNative["setInlineStyles"];
  export const queryElementIds: BobcatNative["queryElementIds"];
  export const supportsStyleProperty: BobcatNative["supportsStyleProperty"];
  export const setInlineStyleProperty: BobcatNative["setInlineStyleProperty"];
  export const removeAttribute: BobcatNative["removeAttribute"];
  export const getAttribute: BobcatNative["getAttribute"];
  export const tagName: BobcatNative["tagName"];
  export const attributeNames: BobcatNative["attributeNames"];
  export const callElementMethod: BobcatNative["callElementMethod"];
  export const getComputedStyleMap: BobcatNative["getComputedStyleMap"];
  export const childElementIds: BobcatNative["childElementIds"];
  export const parentNode: BobcatNative["parentNode"];
  export const childIndex: BobcatNative["childIndex"];
  export const inheritRelevance: BobcatNative["inheritRelevance"];
  export const insertBefore: BobcatNative["insertBefore"];
  export const removeElement: BobcatNative["removeElement"];
  export const replaceElement: BobcatNative["replaceElement"];
  export const swapElement: BobcatNative["swapElement"];
  export const dropElement: BobcatNative["dropElement"];
  export const flushElementTree: BobcatNative["flushElementTree"];
  /**
   * A fresh pipeline id, from one atomic increment. Every realm kind.
   */
  export function generatePipelineId(): number;
  /**
   * `_onPipelineStart`: opens the pipeline's record now. `origin` is a Lynx
   * origin name (`""` for the realm kind's own), `needTimestamps` `0` or
   * `1`; a pipeline that carries none is recorded only while something
   * observes. Every realm kind.
   */
  export function onPipelineStart(
    id: number,
    origin: string,
    dsl: string,
    stage: string,
    needTimestamps: 0 | 1,
  ): void;
  /**
   * `_markTiming`: `mtsRenderStart`/`mtsRenderEnd` are the pipeline's
   * render, every other key a framework mark. Every realm kind.
   */
  export function markPipelineTiming(id: number, key: string): void;
  /** `_bindPipelineIdWithTimingFlag`. Every realm kind. */
  export function bindPipelineTimingFlag(id: number, flag: string): void;
  /**
   * One of the realm's load milestones. In a main-thread realm:
   * `vmExecuteStart` (the load begins, of the bundle at `url`),
   * `vmExecuteEnd` and `mtsRenderStart`; the load ends at the realm's next
   * flush. In a worker realm: `loadCoreEnd`, `loadBackgroundStart` and
   * `loadBackgroundEnd`, the last answering whether the mark completed
   * entries this worker's observers are owed. Any other key does nothing
   * and answers `false`.
   */
  export function markLoadTiming(key: string, url?: string): boolean | undefined;
  /**
   * Main-thread realms: a native update, global-props update or reload
   * arrived — `origin` its Lynx origin name, `flag` its
   * `__lynx_timing_flag` (`""` for none). The realm's next flush binds it.
   */
  export function openNativePipeline(origin: string, flag: string): void;
  /**
   * Main-thread realms: the worker under `key` is this realm's BTS, whose
   * observers hear the pipelines this realm opens.
   */
  export function connectBackground(key: number): void;
  /**
   * Worker realms: the realm has a `lynx.performance` observer (`on` `1`),
   * or no longer has one (`0`), and one of its observers asked for `paint`
   * entries (`paint` `1`) or none did (`0`). Nothing is sent to a worker
   * without an observer, and no `paint` entry to one without a `paint`
   * observer.
   */
  export function observePerformance(on: 0 | 1, paint: 0 | 1): void;
  /**
   * Worker realms: every performance entry this worker is owed, as one
   * record (`bobcat:record`): per entry its pair count, then a key field and
   * a value field per pair, the value `n<number>` or `s<text>`.
   */
  export function takePerformanceEntries(): string;
  /**
   * The `NodeId` of the nested page (`page[part=page]`) the `frame` element
   * `frame` holds in its shadow root now, which belongs to the card rendered
   * under it until `resetFrame` frees it. Read-only. Throws for a node that is not a live frame, and for a frame
   * that sits more than eight frames deep.
   */
  export function framePage(frame: number): number;
  /**
   * Lets go of the card the `frame` element `frame` shows: frees its nested
   * page with its whole subtree, removes every sheet mounted on the frame's
   * shadow root, and builds a fresh nested page, which the next card's
   * `framePage` answers.
   */
  export function resetFrame(frame: number): void;
  /**
   * Retires the `ShadowRealm` child that calls it: the host frees it outside
   * every JavaScript call, whoever still refers to it, and its `dropElement`
   * does nothing from now on. Throws in the page realm, which cannot close
   * itself.
   */
  export function closeRealm(): void;
  export const listenerOpened: BobcatNative["listenerOpened"];
  export const listenerClosed: BobcatNative["listenerClosed"];
  export const timelineMilliseconds: BobcatNative["timelineMilliseconds"];
  export const dispatchEvent: BobcatNative["dispatchEvent"];
  export const switchExposure: BobcatNative["switchExposure"];
  export const setTimer: BobcatNative["setTimer"];
  export const clearTimer: BobcatNative["clearTimer"];
  export const initData: BobcatNative["initData"];
  export const globalProps: BobcatNative["globalProps"];
  export const nativeModuleTable: BobcatNative["nativeModuleTable"];
  /**
   * Fetches `url` the way an image is fetched and answers the id of a
   * `bobcat:future` that settles when the fetch is over: fulfilled with
   * `undefined`, rejected with the host's reason. The realm ending under a
   * wait throws.
   *
   * `true` instead of an id means **already fetched by this view**: nothing
   * was requested and the outcome is known in this very job, which is what a
   * cached fetch looks like from here.
   *
   * Nothing about the bytes comes back and nothing is evaluated. What the
   * host made of them is its own — a fetched Lynx container's sections answer
   * the later loads `lynx.loadScript` and `__AdoptStyleSheet` make by URL.
   */
  export function fetchResource(url: string): number | true;
  /**
   * `url` resolved against the view's base URL by URL rules, in its WHATWG
   * serialization: what a fetch of `url` names, since the view's fetcher
   * resolves a URL it is handed against that same base. An absolute URL
   * answers itself; one that does not resolve throws. Requests nothing.
   *
   * `bobcat:frame` resolves a frame's `src` once, so the card's sections,
   * its background thread and its workers all hang under the URL it fetched.
   */
  export function resolveUrl(url: string): string;
  /**
   * The URL a module at `base` names by `specifier` — the same resolution an
   * `import` there gets, so a `require` and an `import` name one module by one
   * URL. Absolute and relative URLs resolve; a bare name other than a built-in
   * throws, as does a `base` that is not a URL.
   *
   * Loads nothing. `resolveModuleUrl(url, "./")` is that URL's directory, with
   * its trailing slash.
   */
  export function resolveModuleUrl(base: string, specifier: string): string;
  /**
   * Loads the source at `url` and compiles it, before returning.
   *
   * Caching and the `module` object are none of its business: a relative
   * `url` is resolved by the host against the view's base URL by URL rules,
   * an absolute one is taken as it is, and it answers the compiled source;
   * `bobcat:module` is Node's algorithm written over it. The source text never
   * becomes a value in this realm.
   *
   * `parameters` is the parameter list the wrapper of a CommonJS file is
   * compiled with, verbatim; a JSON file is parsed instead, an ES module is
   * linked and evaluated instead, and `parameters` is unread by both. Either
   * way the compile or parse is named by the URL the load answered from, so a
   * `SyntaxError` and every frame beneath it name the file, and a line in the
   * body is the line it sits on in it.
   *
   * An ES module is linked inline: every `import` in it, and in what it
   * imports, is loaded through this same member while it is being compiled,
   * before any body runs. Its evaluation runs no promise jobs, so a graph
   * that awaits at its top level throws, as does a module of a graph that is
   * still evaluating. A module this realm already has — from an `import`, or
   * from an earlier load — is answered from it and evaluated at most once.
   *
   * The call parks the job it runs in until the host answers: the engine
   * thread's tasks keep running, and no other job does — not this realm's
   * promise jobs, and not a realm sharing its thread. A load the host cannot
   * answer throws.
   */
  export function loadModuleSync(
    url: string,
    parameters: string,
  ): LoadedModuleSource;
  /**
   * Waits for the future `id` names, for at most `timeoutMs` milliseconds.
   *
   * True is the future having settled, and `takeFuture` is what reads what it
   * settled to. False is the deadline having passed, which cancels nothing:
   * the operation goes on and this future can be waited on or settled again.
   * A non-finite `timeoutMs` names no deadline at all, and a negative one has
   * already passed.
   *
   * The call parks the job it runs in: the engine thread's tasks keep
   * running, and no other job does — not this realm's promise jobs, and not a
   * realm sharing its thread. The realm ending under the wait throws, and so
   * does a future that is not pending — one already settled and untaken, or
   * one handed to `settleFuture`.
   */
  export function waitFuture(id: number, timeoutMs: number): boolean;
  /**
   * What the future `id` names settled to, removing it.
   *
   * A rejection throws the host's own reason. A future that has not settled —
   * one never waited to completion, or one already taken — throws too.
   */
  export function takeFuture(id: number): unknown;
  /**
   * Asks the host to settle the future `id` names asynchronously: it is
   * awaited on a task of the realm's owner, and delivered by a later call
   * into `bobcat:future`. A future that is not pending throws.
   */
  export function settleFuture(id: number): void;
}

/**
 * The TC39 `ShadowRealm`, a global of every realm this engine builds,
 * installed by the host rather than by ES2023: `bobcat:frame` creates one per
 * card a `frame` loads. Only primitives and callables cross: an object
 * crossing either way is a `TypeError`, and a callable crosses as a wrapped
 * function of the other realm.
 */
declare class ShadowRealm {
  constructor();
  /** Evaluates `sourceText` as a script in the child and answers its value. */
  evaluate(sourceText: string): unknown;
  /**
   * Imports `specifier` in the child and answers its export `bindingName`,
   * which must be a primitive or a callable.
   */
  importValue(specifier: string, bindingName: string): Promise<unknown>;
}

/** What one synchronous load answers with. */
interface LoadedModuleSource {
  /**
   * The URL the load answered from, which a redirect makes different from the
   * URL that was asked for. It is what the source was compiled under, the base
   * a nested `require` resolves against, and `__filename`.
   */
  readonly url: string;
  readonly kind: "commonjs" | "json" | "module";
  /**
   * For `"commonjs"`, the wrapper function: one parameter per name in the
   * `parameters` list, and the file's own body. For `"json"`, the parsed
   * value. For `"module"`, the namespace object of the evaluated module.
   */
  readonly value: unknown;
}

/** One CommonJS, JSON or ES module, as `require.cache` holds it. */
interface RequiredModule {
  /** The URL that was required, which is this entry's key in the cache. */
  readonly id: string;
  /** The same URL: what the module is named by, not where it answered from. */
  readonly filename: string;
  /**
   * What the body left here, or assigned over. JSON is the parsed value, and
   * an ES module is its namespace object — or, when it exports that name,
   * whatever `module.exports` is bound to.
   */
  exports: unknown;
  /**
   * True once the body has returned — from the start for JSON and for an ES
   * module, which was evaluated by the load that answered it. A body that
   * threw leaves no entry behind at all, so no cached module is ever `false`
   * once its `require` has returned.
   */
  readonly loaded: boolean;
}

/**
 * Node's `require`, over the engine's own resource protocol. Loading is
 * synchronous: the call returns with the module evaluated, and no JavaScript
 * runs until the host answers — not this realm's promise jobs, and not a
 * realm sharing its thread.
 */
interface Require {
  /**
   * The exports of the module `specifier` names, evaluated here if this realm
   * has not evaluated it already. A specifier this engine cannot resolve —
   * a bare name, or anything the normalizer refuses — is a `TypeError`
   * carrying its message, where Node reports `MODULE_NOT_FOUND`.
   */
  (specifier: string): unknown;
  /**
   * The URL `specifier` names — the cache key. Loads nothing, and refuses
   * what a `require` of the same specifier would, with the same `TypeError`.
   */
  resolve(specifier: string): string;
  /**
   * The realm's CommonJS cache, shared by every `require` in it. Deleting a
   * key makes the next `require` load and evaluate that URL again.
   */
  readonly cache: Record<string, RequiredModule | undefined>;
}

declare module "bobcat-internal:worker" {
  export const postWorkerMessage: BobcatWorkerNative["postWorkerMessage"];
  export const closeWorker: BobcatWorkerNative["closeWorker"];
  export const workerName: BobcatWorkerNative["workerName"];
  export const sharedObject: BobcatWorkerNative["sharedObject"];
}

/**
 * The embedder's native modules as a realm reaches them. Every realm kind
 * declares this module with the same one member, the MTS realm included.
 * Which modules a realm's `NativeModules` names is not here: the MTS realm
 * reads the view's table from `bobcat-internal:host` and posts it to its BTS
 * Worker in `initialize`. `bobcat:native-modules` is the transport written
 * over `invokeNativeModule`.
 */
declare module "bobcat-internal:native-modules" {
  /**
   * Hands one `NativeModules.<module>.<method>(...)` call to the embedder's
   * module of that name, and returns at once: a module answers through the
   * callbacks among its arguments, never through a result.
   *
   * `call` is this realm's own number for the call, which a callback's answer
   * carries back. `args` is the argument list as JSON array text, with each
   * function argument written as `null`; `callbacks` names those arguments by
   * index, joined with commas and empty when there are none. The host mints
   * one single-shot callback per index, and each is answered — or released
   * unanswered — through `bobcat:native-modules`'s
   * `__BobcatNativeModuleCallback`, in the realm that made the call.
   *
   * A module the view does not carry is not an error here, and neither is a
   * method its module did not declare: the call is dropped and its callbacks
   * released.
   */
  export function invokeNativeModule(
    call: number,
    module: string,
    method: string,
    args: string,
    callbacks: string,
  ): void;
}
