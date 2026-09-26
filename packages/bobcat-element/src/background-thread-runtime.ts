// Imported for their effects, first: this module calls
// `scope.addEventListener` and reads the timer globals as it is evaluated,
// so the global scope and the timers are in place before it runs.
import "bobcat:worker";
import "bobcat:timers";
import { callNativeModule } from "bobcat:native-modules";
import type { WorkerGlobalScope } from "bobcat:worker";
import { splitRecord } from "bobcat:record";
import {
  type ContextEvent,
  createCrossThreadContext,
} from "bobcat:cross-thread-context";
import { SelectorQuery, type SendQuery } from "bobcat:selector-query";
import { createLynxModules } from "bobcat:lynx-modules";
import { type BundleHandle, createBundleFetches } from "bobcat:bundle-fetch";
import { GlobalEventEmitter } from "bobcat:global-event-emitter";
import { console, reportError } from "bobcat:diagnostics";
import type { TimerGlobals } from "bobcat:timers";
import { cancelAnimationFrame, clearAnimationFrames, requestAnimationFrame } from "bobcat:animation-frame";
import { createSystemInfo } from "bobcat:system-info";

const timers = globalThis as unknown as TimerGlobals;

// The bobcat:bts bootstrap and every card body's `BTS_CHUNK_PREAMBLE` import
// this runtime. Like MTS, lynx is a module binding, never a global property.
// The bootstrap installs a message receiver, then returns so Worker messages
// can initialize the runtime before the application entry is imported.
const scope = globalThis as unknown as WorkerGlobalScope;

const coreContext = createCrossThreadContext();

type AppHook = (...args: unknown[]) => unknown;
const emitter = new GlobalEventEmitter();
const jsModules = new Map<string, unknown>([["GlobalEventEmitter", emitter]]);
// The embedder's `NativeModule`s, as the objects a card calls: one property
// per module the view was built with, filled in by `__BobcatInitializeBTS`
// out of the table the `initialize` message carries, before the entry
// imports. A plain `Worker` is posted no `initialize`, so its object stays
// empty. A module the host does not have is `undefined` — web-core's answer,
// a missing key on the object `createNativeModules` builds, where native
// answers `null`.
//
// The property name `nativeModuleProxy` stays although nothing is a Proxy any
// more: it is the name ReactLynx reads (`nativeApp.nativeModuleProxy
// .LynxUIMethodModule`), and renaming it would only break that lookup.
const nativeModules: Record<string, object> = {};
const app: {
  NativeModules: object;
  _apiList: object;
  // Names web-core's chunk wrapper passes off the app object
  // (`createBundleInitReturnObj`). Nothing in this realm ever sets one — a
  // card's `Card`/`Component` come from lynx-core, which this engine does not
  // run — so each stays `undefined`, and so does the export
  // `BTS_CHUNK_PREAMBLE` imports it by.
  Card?: unknown;
  Component?: unknown;
  nativeAppId?: unknown;
  Behavior?: unknown;
  LynxJSBI?: unknown;
  define?: Function;
  require?: Function;
  _nativeApp?: object;
  lynx?: object;
  OnLifecycleEvent?: AppHook;
  publishEvent?: AppHook;
  publicComponentEvent?: AppHook;
  callDestroyLifetimeFun?: AppHook;
  updateGlobalProps?: AppHook;
  updateCardData?: AppHook;
  onAppReload?: AppHook;
  processCardConfig?: AppHook;
  _params: {initData: unknown; updateData: unknown; processorName: string; cacheData: unknown[]};
  GlobalEventEmitter: GlobalEventEmitter;
  registerModule(name: string, value: unknown): void;
  getJSModule(name: string): unknown;
} = {
  NativeModules: nativeModules,
  _apiList: {},
  _params: {initData: null, updateData: undefined, processorName: "", cacheData: []},
  GlobalEventEmitter: emitter,
  registerModule(name, value) { jsModules.set(name, value); },
  getJSModule(name) { return jsModules.get(name); },
};
const destructionRegistry = new FinalizationRegistry<() => unknown>(callback => callback());

// Looked up by the id a `callLepusMethodResult` carries, which a result for
// a call made without a callback lacks.
const callbacks: Map<number | undefined, (result: unknown) => void> =
  new Map();
let nextCallbackId = 1;

/**
 * What the main thread sends this realm: a runtime call, tagged
 * `bobcat: "runtime"`, or a Context event's public fields, which carry no tag.
 */
type FromMainThread =
  | ({ bobcat: "runtime"; method: "initialize" } & InitializeOptions)
  | {
      bobcat: "runtime";
      method: "publishEvent" | "publicComponentEvent" | "updateGlobalProps" | "updateCardData" | "onAppReload" | "processCardConfig";
      args: unknown[];
    }
  | { bobcat: "runtime"; method: "dispose" }
  | {
      bobcat: "runtime";
      method: "callLepusMethodResult";
      id?: number;
      result?: unknown;
      error?: { name: string; message: string };
    }
  | { bobcat: "runtime"; method: "nodeQueryResult" | "reloadResult"; id?: number; result?: unknown }
  | { bobcat: "runtime"; method: "sendGlobalEvent"; name: string; args: unknown[] }
  | (ContextEvent & { bobcat?: never });

/**
 * web-core registers these two handlers lazily: an event received before the
 * framework installs its hook waits for that hook. Later calls read the current
 * property because ReactLynx replaces the initial handler during setup.
 */
function createPublishHandler(name: "publishEvent" | "publicComponentEvent") {
  let handler: AppHook | undefined;
  let pending: unknown[][] = [];
  async function replay(args: unknown[]) {
    // As in web-core's async RPC receiver, call the current hook immediately,
    // but report a rejection independently so later queued events still run.
    return app[name]?.apply(app, args);
  }
  Object.defineProperty(app, name, {
    get() { return handler; },
    set(value: AppHook | undefined) {
      handler = value;
      if (typeof value === "function") {
        const queued = pending;
        pending = [];
        for (const args of queued) {
          void replay(args);
        }
      }
    },
  });
  return (args: unknown[]) => {
    const current = app[name];
    if (typeof current === "function") {
      current.apply(app, args);
    } else {
      pending.push(args);
    }
  };
}

const publishEvent = createPublishHandler("publishEvent");
const publicComponentEvent = createPublishHandler("publicComponentEvent");

const sendQuery: SendQuery = (operation, token, params, callback) => {
  // These are native BTS argument checks, before any asynchronous delivery.
  if (typeof token.identifier !== "string" || typeof token.component_id !== "string") {
    throw new TypeError("node identifier and component must be strings");
  }
  let id;
  if (callback) { id = nextCallbackId++; callbacks.set(id, callback); }
  try { scope.postMessage({bobcat: "runtime", method: "nodeQuery", operation, token, params, id}); }
  catch (error) { if (id !== undefined) callbacks.delete(id); throw error; }
};

const nativeApp = {
  nativeModuleProxy: nativeModules,
  /**
   * One bundle script, loaded through the host, as the `{init}` object
   * web-core's `createBundleInitReturnObj` answers with: `init` answers the
   * module's exports. It writes neither of `requireModule`'s caches, as
   * lynx-core's own `loadScript` writes neither.
   *
   * `loadScriptAsync` and `readScript` are deliberately absent: the second
   * would hand a source's text to JavaScript, which nothing in this engine
   * does.
   */
  loadScript(sourceURL: string, entryName?: string) {
    return modules.loadScriptInit(sourceURL, entryName);
  },
  createJSObjectDestructionObserver(callback: () => unknown): object {
    const observer = {};
    destructionRegistry.register(observer, callback);
    return observer;
  },

  callLepusMethod(
    name: string,
    data: unknown,
    callback?: (result: unknown) => void,
  ) {
    if (arguments.length < 2) throw new TypeError("callLepusMethod requires name and data");
    if (typeof name !== "string") name = "";
    if (data === null || typeof data !== "object") return;
    let id;
    if (typeof callback === "function") {
      id = nextCallbackId++;
      callbacks.set(id, callback);
    }
    try {
      scope.postMessage({
        bobcat: "runtime", method: "callLepusMethod", name, data, id,
      });
    } catch (error) {
      if (id !== undefined) callbacks.delete(id);
      throw error;
    }
  },
};

// web-worker-rpc callbackify invokes callbacks in a Promise continuation.
// Release the ID before scheduling it, so duplicate replies cannot invoke it twice.
async function receiveLepusResult(
  message: Extract<FromMainThread, { method: "callLepusMethodResult" }>,
) {
  const callback = callbacks.get(message.id);
  callbacks.delete(message.id);
  await undefined;
  if (message.error !== undefined) {
    const error = new Error(message.error.message);
    error.name = message.error.name;
    throw error;
  }
  return callback?.(message.result);
}

coreContext.addEventListener(
  "__OnLifecycleEvent",
  (event: { data: unknown }) => {
    app.OnLifecycleEvent?.call(app, event.data);
  },
);

coreContext.connect((event) => scope.postMessage({ type: event.type, data: event.data, origin: event.origin }));
// Only the built-in BTS bootstrap arms initialization. Its first Worker
// message supplies inputs; ordinary messages wait for the entry's imports to
// settle, whether they finished or threw. Nothing outside this realm waits on
// that: the view is ready once MTS has booted, whatever becomes of the BTS.
let startBackground: ((options: InitializeOptions) => Promise<void>) | undefined;
let entryReady: Promise<void> | undefined;

function noop() {
  return undefined;
}

/**
 * Arms this realm as the BTS: the first `initialize` message initializes it
 * and then calls `loadEntry` with that message, which is where `bobcat:bts`
 * reads the entry's URL from.
 */
export function __BobcatStartBTS(loadEntry: (options: InitializeOptions) => Promise<unknown>) {
  startBackground = async options => {
    __BobcatInitializeBTS(options);
    await loadEntry(options);
  };
}

scope.addEventListener("message", (event: { data: FromMainThread }): void | Promise<void> => {
  const message = event.data;
  // Disposal must remain deliverable while entry imports are outstanding:
  // the released view can no longer provide their resources. Clean up any
  // hook already installed, then acknowledge through the ordinary Worker.
  if (message?.bobcat === "runtime" && message.method === "dispose") {
    return dispose();
  }
  if (message?.bobcat === "runtime" && message.method === "initialize") {
    const start = startBackground;
    if (start) {
      startBackground = undefined;
      const started = start(message);
      // An entry that throws is a worker script that throws: reported at the
      // parent Worker's `error` event, with this realm still up and taking
      // messages, as HTML's "run a worker" leaves it. Nothing waits on the
      // outcome — the messages behind this one wait only for it to settle.
      started.catch(error => scope.reportError(error));
      entryReady = started.then(noop, noop).then(() => {
        entryReady = undefined;
      });
    }
    return;
  }
  if (entryReady) return entryReady.then(() => receiveMessage(message));
  return receiveMessage(message);
});

function receiveMessage(message: FromMainThread): void | Promise<void> {
  if (message?.bobcat !== "runtime") {
    coreContext.receive(message);
    return;
  }
  switch (message.method) {
    case "publishEvent":
      publishEvent(message.args);
      break;
    case "publicComponentEvent":
      publicComponentEvent(message.args);
      break;
    case "reloadResult":
    case "nodeQueryResult": {
      const callback = callbacks.get(message.id);
      try { callback?.(message.result); }
      finally { callbacks.delete(message.id); }
      break;
    }
    case "updateGlobalProps":
    case "updateCardData":
    case "onAppReload":
    case "processCardConfig":
      app[message.method]?.apply(app, message.args);
      break;
    case "sendGlobalEvent":
      emitter.emit(message.name, message.args);
      break;
    case "callLepusMethodResult":
      return receiveLepusResult(message);
  }
}

async function dispose() {
  clearAnimationFrames();
  // `lynx.reportError`, not the global scope's. That one is reported when this
  // entry's checkpoint returns its rejection, after the `disposed` reply below
  // has been posted, and the main thread terminates this Worker on that reply,
  // so the report would reach no one. This one reaches the host during the
  // call.
  try { app.callDestroyLifetimeFun?.call(app); }
  catch (error) { lynx.reportError(error); }
  // Match web-worker-rpc's await boundary before replying. This does not
  // wait for asynchronous work launched by the framework's synchronous hook.
  await undefined;
  scope.postMessage({ bobcat: "runtime", method: "disposed" });
}

// The runtime constants until `initialize` hands this realm the MTS realm's
// own `SystemInfo`, screen included. A plain `Worker` that imports this module
// is posted no `initialize`, and reports the constants alone. The raw BTS
// environment has the same object as a global, beside this export and
// `lynx.SystemInfo`.
export let SystemInfo = createSystemInfo();

// The realm's one console, which `bobcat:worker` also installs as the global
// `console`: a raw BTS entry's import and a bundle body's preamble binding
// name the same object as the global.
export { console };

// The bundle's base URL is registered before the app-service entry loads.
export const lynx = {
  setTimeout: timers.setTimeout,
  setInterval: timers.setInterval,
  clearTimeout: timers.clearTimeout,
  clearInterval: timers.clearInterval,
  Promise: globalThis.Promise,
  queueMicrotask(callback: () => void) {
    if (typeof callback !== "function") throw new TypeError("queueMicrotask requires a function");
    // A callback that throws is reported the way an uncaught exception is.
    void Promise.resolve().then(() => {
      try { callback(); } catch (error) { scope.reportError(error); }
    });
  },
  // `bobcat:animation-frame`'s, which runs them on this worker's vsync: IDs
  // and errors stay on BTS, and no MTS message or acknowledgement takes part.
  // A callback that throws is an uncaught exception of this worker realm, as
  // it is in a browser worker: the global scope's `reportError` reports it at
  // the parent `Worker` and to the embedder, and the frame's other callbacks
  // run.
  requestAnimationFrame,
  cancelAnimationFrame,
  reload(value?: unknown, callback?: unknown) {
    // Native only parses object arguments. Primitives (including null) mean
    // an empty data table; arrays/functions do not produce a reload table.
    let data = {};
    if (typeof value === "function") return;
    if (value !== null && typeof value === "object") {
      data = value;
      if (Array.isArray(data)) return;
    }
    let id;
    if (typeof callback === "function") {
      id = nextCallbackId++;
      callbacks.set(id, () => callback());
    }
    try { scope.postMessage({bobcat:"runtime", method:"reloadFromJS", data, id}); }
    catch (error) { if (id !== undefined) callbacks.delete(id); throw error; }
  },
  SystemInfo,
  __initData: {} as unknown,
  __globalProps: {} as unknown,
  getJSModule: app.getJSModule,
  registerModule: app.registerModule,
  // A leveled diagnostic, reported by this realm directly: no `error` event,
  // and nothing sent to the main thread.
  reportError,

  createSelectorQuery(component?: string) { return new SelectorQuery(sendQuery, error => lynx.reportError(error), component); },
  getApp() {
    return app;
  },
  getNativeApp() {
    return nativeApp;
  },
  // `options` reaches the modules table and is ignored there; see
  // `requireModule` in `bobcat:lynx-modules` for why native ignores it too.
  requireModule(path: string, entry?: string, options?: {timeout?: number}) {
    return modules.requireModule(path, entry, options);
  },
  loadScript(key: string, options: {bundleName?: string}) { return modules.loadScript(key, options); },
  // `options` is accepted and ignored, as native ignores it.
  fetchBundle(url: string, _options?: unknown): BundleHandle {
    return bundleFetches.fetchBundle(url, _options);
  },
  getCoreContext() {
    return coreContext;
  },
};

/**
 * This realm's `lynx.fetchBundle`.
 *
 * `later` is a **posted task**, not an inline call: on the background thread
 * native hands a settled fetch to its own mediator, so a callback registered
 * on a handle that has already settled runs after the current task rather
 * than inside it. MTS is the other way round; see `bundle-fetch.ts`.
 */
const bundleFetches = createBundleFetches({
  report: error => { scope.reportError(error); },
  later: run => { lynx.setTimeout(run, 0); },
});

const modules = createLynxModules(app, lynx, console);
app.define = modules.define;
app.require = modules.require;
app._nativeApp = nativeApp;
app.lynx = lynx;
export const lynxCoreInject = {tt: app};
export const globDynamicComponentEntry = "__Card__";
Object.assign(scope, {globDynamicComponentEntry});

// The rest of web-core's chunk parameter list, as module exports:
// `bobcat-source` prepends `BTS_CHUNK_PREAMBLE` to every card body it
// registers for this realm — a container's bodies and an XML page's
// background-thread script — and that preamble imports these names from here.
// Each is this realm's value for the parameter of that name web-core's chunk
// wrapper passes (`createBundleInitReturnObj`).
export const NativeModules = nativeModules;
export const Card = app.Card;
export const Component = app.Component;
export const nativeAppId = app.nativeAppId;
export const Behavior = app.Behavior;
export const LynxJSBI = app.LynxJSBI;
export const setTimeout = lynx.setTimeout;
export const setInterval = lynx.setInterval;
export const clearTimeout = lynx.clearTimeout;
export const clearInterval = lynx.clearInterval;
export { requestAnimationFrame, cancelAnimationFrame };

/**
 * One entry's bundle: the URL its template answered from, and nothing else.
 *
 * Every body the container carried is a registered resource of its own, beside
 * that URL, and nothing is loaded here — `lynx.requireModule`,
 * `lynx.loadScript` and `nativeApp.loadScript` each build the URL their path
 * names and load it synchronously, as MTS's `__LoadLepusChunk` does. The
 * template URL is the base they resolve against: no URL means no base, and
 * then only an absolute path resolves.
 */
export function __BobcatRegisterBundle(templateUrl?: string, entry?: string) {
  modules.registerTemplateUrl(templateUrl, entry);
}

/** The page's data, its global props and the processor name. */
interface BackgroundData {
  initData?: unknown;
  updateData?: unknown;
  globalProps?: unknown;
  processorName?: string;
  cacheData?: unknown[];
}

/**
 * What the main thread's `initialize` message opens this realm with: the
 * page's data, the BTS entry's URL — already absolute, `undefined` for a view
 * that named none — the MTS realm's own `SystemInfo`, and the embedder's
 * modules as the record the host wrote, two fields per module.
 */
interface InitializeOptions extends BackgroundData {
  entry?: string;
  systemInfo?: Record<string, unknown>;
  nativeModuleTable?: string;
}

export function __BobcatInitializeBTS(options: InitializeOptions) {
  const params = options;
  app._params = { initData:params.initData ?? null, updateData:params.updateData, processorName:params.processorName ?? "", cacheData:params.cacheData ?? [] };
  lynx.__initData = Object.hasOwn(params, "updateData") ? params.updateData : params.initData;
  lynx.__globalProps = params.globalProps || {};
  // Into the one object `NativeModules`, `app.NativeModules` and
  // `nativeModuleProxy` all name.
  Object.assign(nativeModules, nativeModulesFromTable(params.nativeModuleTable ?? ""));
  SystemInfo = createSystemInfo(params.systemInfo);
  lynx.SystemInfo = SystemInfo;
  // Keep the raw BTS environment consistent with its module binding.
  Object.assign(scope, { SystemInfo });
}

/**
 * The `NativeModules` members out of the module table `initialize` carries,
 * as the host wrote it: two record fields per module, its name and then its
 * method names joined with commas.
 *
 * Every declared method, and nothing else: a method a module did not declare
 * is `undefined`, which is what native answers for one its module does not
 * carry. Each returns `undefined` — a module answers through the callbacks
 * among its arguments, never through a return value.
 */
function nativeModulesFromTable(table: string): Record<string, object> {
  const fields = splitRecord(table);
  const modules: Record<string, object> = {};
  for (let index = 0; index + 1 < fields.length; index += 2) {
    const name = fields[index]!;
    const methods = fields[index + 1]!;
    modules[name] = Object.fromEntries((methods === "" ? [] : methods.split(",")).map(method =>
      [method, (...args: unknown[]) => callNativeModule(name, method, args)]));
  }
  return modules;
}

__BobcatInitializeBTS({});
