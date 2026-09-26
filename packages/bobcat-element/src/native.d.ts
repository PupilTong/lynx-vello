/**
 * The native functions exported by `bobcat-internal:host`. They speak DOM
 * vocabulary over numeric `NodeId`s and own the document and style/layout
 * commit; misuse crashes at this boundary instead of being validated.
 */
interface BobcatNative {
  /**
   * Builds the realm's one document out of the four page switches, in
   * `PageConfig`'s order, and the view's own resources, which never reach
   * this realm: its metrics, its fonts and its style pool.
   *
   * It never waits. The view's author stylesheets are not mounted here: the
   * first `flushElementTree` mounts them, in the order the view listed them.
   * A switch that is not a boolean, and a realm that already has a document,
   * both throw.
   */
  createDocument(
    defaultDisplayLinear: boolean,
    defaultOverflowVisible: boolean,
    enableCssSelector: boolean,
    enableJSDataProcessor: boolean,
  ): void;
  /** Marks the permanent page live and returns its `NodeId`. */
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
   * Dispatches one Lynx UI method by name on the element and answers its
   * result as text, or `null` when the engine has no method of that name.
   *
   * `boundingClientRect` answers `"<left>,<top>,<width>,<height>"` — the
   * border box in viewport CSS px as of the last completed layout pass,
   * ancestor scroll offsets applied, transforms ignored (native's own
   * conversion ignores them too), zeros for an element with no box.
   *
   * Runs no style, layout or paint: it reports the last completed pass, and
   * user code decides when to flush.
   */
  callElementMethod(nodeId: number, method: string): string | null;
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
   * — the text node a `raw-text` reflects — goes with it.
   */
  dropElement(nodeId: number): void;
  /**
   * Commits pending mutations through style and layout.
   *
   * The first call waits for every author stylesheet the view listed and
   * mounts them in listed order before anything is styled; a sheet that
   * failed to load throws `loading stylesheet <url>: <reason>`. Before a
   * painter has bound the view it then waits for the binding.
   */
  flushElementTree(): void;
  /**
   * Records that something in the realm is now registered for `eventName`,
   * where nothing was — the first registration anywhere in the document.
   * The painting side routes against that name set; the host keeps no
   * per-element listener index at all, and never hears a second
   * registration for a name already open.
   */
  listenerNameOpened(eventName: string): void;
  /** The reverse: the last registration for that name anywhere went away. */
  listenerNameClosed(eventName: string): void;
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
   * unread to its BTS Worker in `initialize`. Answers once, like `initData`.
   */
  nativeModuleTable(): string;
}

/**
 * The native functions a worker realm gets instead of the document, under
 * `bobcat-internal:worker`. Its `bobcat-internal:host` carries only the
 * members every realm is opened with — `requestScriptFrame`, the timer pair,
 * the `Future` members, `fetchResource`, `resolveModuleUrl`,
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
   * named none. `bobcat:worker` reads it as it is evaluated. Answers once:
   * the string is handed over, not kept.
   */
  workerName(): string;
}

declare module "bobcat-internal:host" {
  /** Initial processor name, handed over once as a plain string. */
  export function initialProcessor(): string | undefined;
  export function requestScriptFrame(pending: boolean): void;
  export function preloadStyleSheet(url: string): void;
  export function adoptStyleSheet(url: string): void;
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
   * The host joins `url` to `baseUrl` — the page entry's response URL, which
   * the realm holds — by URL rules, and does not keep the base. A `url` that
   * does not resolve starts nothing and answers `null`.
   */
  export function createWorker(
    url: string,
    name: string,
    baseUrl: string,
  ): string | null;
  /**
   * Posts one message to that worker. Any value the host boundary carries; a
   * value the engine's serializer refuses throws at this call.
   */
  export function sendWorkerMessage(key: string, data: unknown): void;
  export function terminateWorker(key: string): void;
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
  export const insertBefore: BobcatNative["insertBefore"];
  export const removeElement: BobcatNative["removeElement"];
  export const replaceElement: BobcatNative["replaceElement"];
  export const swapElement: BobcatNative["swapElement"];
  export const dropElement: BobcatNative["dropElement"];
  export const flushElementTree: BobcatNative["flushElementTree"];
  export const listenerNameOpened: BobcatNative["listenerNameOpened"];
  export const listenerNameClosed: BobcatNative["listenerNameClosed"];
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
