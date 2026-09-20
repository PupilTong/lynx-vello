import { callNativeModule } from "bobcat:worker";
import { requestScriptFrame } from "bobcat-internal:host";
import type { WorkerGlobalScope } from "bobcat:worker";
import {
  type ContextEvent,
  createCrossThreadContext,
} from "bobcat:cross-thread-context";
import { SelectorQuery, type SendQuery } from "bobcat:selector-query";
import { createLynxModules } from "bobcat:lynx-modules";
import { GlobalEventEmitter } from "bobcat:global-event-emitter";
import type { TimerGlobals } from "bobcat:timers";

const timers = globalThis as unknown as TimerGlobals;

// The bobcat:bts bootstrap and the BTS application's entry preamble import
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
// before the entry imports. A module the host does not have is `undefined`
// — web-core's answer, a missing key on the object `createNativeModules`
// builds, where native answers `null`.
//
// The property name `nativeModuleProxy` stays although nothing is a Proxy any
// more: it is the name ReactLynx reads (`nativeApp.nativeModuleProxy
// .LynxUIMethodModule`), and renaming it would only break that lookup.
const nativeModules: Record<string, object> = {};
const app: {
  NativeModules: object;
  _apiList: object;
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
const animationCallbacks = new Map<number, (milliseconds: number) => void>();
let nextAnimationId = 1;

/**
 * What the main thread sends this realm: a runtime call, tagged
 * `bobcat: "runtime"`, or a Context event's public fields, which carry no tag.
 */
type FromMainThread =
  | ({ bobcat: "runtime"; method: "initialize" } & BackgroundData & {
      systemInfo?: Record<string, unknown>;
      nativeModules?: Record<string, string[]>;
    })
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
   * One bundle script, loaded through the registered sources or through the
   * host, as the `{init}` object web-core's `createBundleInitReturnObj`
   * answers with: `init` answers the module's exports. It writes neither of
   * `requireModule`'s caches, as lynx-core's own `loadScript` writes neither.
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

export function __BobcatStartBTS(loadEntry: () => Promise<unknown>) {
  startBackground = async options => {
    __BobcatInitializeBTS(options);
    await loadEntry();
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
  animationCallbacks.clear();
  updateFrameRequest();
  try { app.callDestroyLifetimeFun?.call(app); }
  catch (error) { lynx.reportError(error); }
  // Match web-worker-rpc's await boundary before replying. This does not
  // wait for asynchronous work launched by the framework's synchronous hook.
  await undefined;
  scope.postMessage({ bobcat: "runtime", method: "disposed" });
}

// The selected runtime target, independent of the compiler's minimum SDK.
export let SystemInfo: Readonly<Record<string, unknown>> = Object.freeze({
  platform: "headless", runtimeType: "quickjs", lynxSdkVersion: "4.1.0",
});

function printable(value: unknown): string {
  if (value instanceof Error) {
    const summary = String(value);
    return value.stack?.includes(summary) ? value.stack
      : value.stack ? `${summary}\n${value.stack}` : summary;
  }
  if (typeof value === "string") return value;
  try { return JSON.stringify(value) ?? String(value); }
  catch { return String(value); }
}

let frameRequested = false;
function updateFrameRequest() {
  const pending = animationCallbacks.size > 0;
  if (pending === frameRequested) return;
  frameRequested = pending;
  requestScriptFrame(pending);
}

// Called while handling the painter's Vsync message. Callback IDs
// and errors stay on BTS; no MTS message or acknowledgement participates.
export function __BobcatBeginFrame(milliseconds: number) {
  frameRequested = false;
  const ids = Array.from(animationCallbacks.keys());
  for (const id of ids) {
    const callback = animationCallbacks.get(id);
    animationCallbacks.delete(id);
    if (callback) {
      try { callback(milliseconds); }
      catch (error) { lynx.reportError(error); }
    }
  }
  updateFrameRequest();
}

export const console = Object.fromEntries(
  ["log", "info", "debug", "warn", "error"].map(level => [level,
    (...args: unknown[]) => scope.postMessage({ bobcat: "runtime", method: "console", level,
      message: args.map(printable).join(" ") }),
  ]),
);

// Compiler modules are registered before the app-service entry executes.
export const lynx = {
  setTimeout: timers.setTimeout,
  setInterval: timers.setInterval,
  clearTimeout: timers.clearTimeout,
  clearInterval: timers.clearInterval,
  Promise: globalThis.Promise,
  queueMicrotask(callback: () => void) {
    if (typeof callback !== "function") throw new TypeError("queueMicrotask requires a function");
    void Promise.resolve().then(() => {
      try { callback(); } catch (error) { lynx.reportError(error); }
    });
  },
  requestAnimationFrame(callback: (milliseconds: number) => void) {
    if (typeof callback !== "function") throw new TypeError("requestAnimationFrame requires a function");
    const id = nextAnimationId++;
    animationCallbacks.set(id, callback);
    updateFrameRequest();
    return id;
  },
  cancelAnimationFrame(id: number) {
    animationCallbacks.delete(id);
    updateFrameRequest();
  },
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
  reportError(error: unknown, options?: {level?: string}) {
    const level = options?.level;
    scope.postMessage({ bobcat: "runtime", method: "reportError",
      level: level === "warning" || level === "fatal" ? level : "error",
      message: printable(error) });
  },

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
  getCoreContext() {
    return coreContext;
  },
};

const modules = createLynxModules(app, lynx, console);
app.define = modules.define;
app.require = modules.require;
app._nativeApp = nativeApp;
app.lynx = lynx;
export const lynxCoreInject = {tt: app};
export const globDynamicComponentEntry = "__Card__";
Object.assign(scope, {globDynamicComponentEntry});

/**
 * The bundle one entry was decoded from: its module sources, its named
 * sections, and the URL its template answered from, which is the base a path
 * no manifest carries is resolved against. No URL means no base.
 */
export function __BobcatRegisterBundle(manifest: Record<string, string>, wrapped: boolean,
  sections: Record<string, string> = {}, entry?: string, templateUrl?: string) {
  modules.register(manifest, wrapped, entry);
  modules.registerSections(sections, entry);
  modules.registerTemplateUrl(templateUrl, entry);
}

interface BackgroundData {
  initData?: unknown;
  updateData?: unknown;
  globalProps?: unknown;
  processorName?: string;
  cacheData?: unknown[];
}

/**
 * What the main thread opens this realm with: the page's data, the runtime
 * target, and the modules the embedder injected at `create_lynx_view`, as
 * `{name: methods}`.
 */
interface InitializeOptions extends BackgroundData {
  systemInfo?: Record<string, unknown>;
  nativeModules?: Record<string, string[]>;
}

export function __BobcatInitializeBTS(options: InitializeOptions) {
  const params = options;
  app._params = { initData:params.initData ?? null, updateData:params.updateData, processorName:params.processorName ?? "", cacheData:params.cacheData ?? [] };
  lynx.__initData = Object.hasOwn(params, "updateData") ? params.updateData : params.initData;
  lynx.__globalProps = params.globalProps || {};
  // Every declared method, and nothing else: a method a module did not
  // declare is `undefined`, which is what native answers for one its module
  // does not carry. Each returns `undefined` — a module answers through the
  // callbacks among its arguments, never through a return value.
  for (const [name, methods] of Object.entries(options.nativeModules ?? {})) {
    nativeModules[name] = Object.fromEntries(methods.map(method =>
      [method, (...args: unknown[]) => callNativeModule(name, method, args)]));
  }
  if (options.systemInfo) SystemInfo = Object.freeze({ ...SystemInfo, ...options.systemInfo });
  lynx.SystemInfo = SystemInfo;
  // Keep the raw BTS environment consistent with its module snapshot.
  Object.assign(scope, { SystemInfo });
}

__BobcatInitializeBTS({});
