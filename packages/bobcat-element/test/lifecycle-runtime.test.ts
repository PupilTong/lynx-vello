import { afterAll, beforeAll, describe, expect, it, rstest } from "@rstest/core";
import * as eventTarget from "../src/event-target.ts";
import * as crossThreadContext from "../src/cross-thread-context.ts";
import type * as btsRuntime from "../src/background-thread-runtime.ts";
import type * as mtsRuntime from "../src/main-thread-runtime.ts";
import type { Worker } from "../src/worker.ts";
import * as selectorQuery from "../src/selector-query.ts";
import type * as lynxModules from "../src/lynx-modules.ts";
import type * as animationFrame from "../src/animation-frame.ts";
import * as systemInfo from "../src/system-info.ts";
// Answered lazily, as `bobcat:worker` is in native-modules.test.ts: the modules
// table and the animation-frame module import `bobcat-internal:host`, whose
// replacement below is built out of this file's own bindings, so neither may
// be required above them. Both runtimes import the one animation-frame
// instance here, where each realm has its own.
let moduleTable: typeof lynxModules;
let animationFrames: typeof animationFrame;
rstest.mockRequire("bobcat:lynx-modules", () => moduleTable);
rstest.mockRequire("bobcat:animation-frame", () => animationFrames);
rstest.mockRequire("bobcat:system-info", () => systemInfo);
import * as globalEventEmitter from "../src/global-event-emitter.ts";
rstest.mockRequire("bobcat:global-event-emitter", () => globalEventEmitter);
rstest.mockRequire("bobcat:selector-query", () => selectorQuery);
const queryNodes = rstest.fn();
rstest.mockRequire("bobcat:element", () => ({ __BobcatQueryNodes: queryNodes }));

rstest.mockRequire("bobcat:event-target", () => eventTarget);
rstest.mockRequire("bobcat:cross-thread-context", () => crossThreadContext);
// This suite drives no native module; the BTS runtime only needs the
// transport to exist, because `callNativeModule` is what its method wrappers
// close over.
rstest.mockRequire("bobcat:worker", () => ({ callNativeModule: rstest.fn() }));
rstest.mockRequire("bobcat:timers", () => ({}));
import type * as sectionUrl from "../src/section-url.ts";
import type * as bundleFetch from "../src/bundle-fetch.ts";
import type * as future from "../src/future.ts";
import type * as diagnosticsModule from "../src/diagnostics.ts";
// Answered lazily for the reason `bobcat:lynx-modules` is: all four import
// `bobcat-internal:host`, whose replacement below is built out of this file's
// own bindings. Both runtimes report through the one `bobcat:diagnostics`, so
// every diagnostic of either reaches the two recorders below directly.
let sectionUrls: typeof sectionUrl;
let bundleFetches: typeof bundleFetch;
let futures: typeof future;
let diagnostics: typeof diagnosticsModule;
rstest.mockRequire("bobcat:section-url", () => sectionUrls);
rstest.mockRequire("bobcat:bundle-fetch", () => bundleFetches);
rstest.mockRequire("bobcat:future", () => futures);
rstest.mockRequire("bobcat:diagnostics", () => diagnostics);
const requestScriptFrame = rstest.fn();
const preloadStyleSheet = rstest.fn();
const adoptStyleSheet = rstest.fn();
const reportedErrors = rstest.fn();
const consoleMessages = rstest.fn();
// The runtime reads the view's page data as it evaluates; this view has none.
rstest.mockRequire("bobcat-internal:host", () => ({
  requestScriptFrame,
  reportScriptError: reportedErrors,
  logScriptMessage: consoleMessages,
  preloadStyleSheet, adoptStyleSheet,
  initialProcessor: () => "",
  initData: () => undefined,
  globalProps: () => undefined,
  nativeModuleTable: () => "",
  // The two members every synchronous load is written over, stood in for as
  // in lynx-modules.test.ts: Node's own `URL`, and `new Function` for the
  // wrapper the engine compiles a body in.
  resolveModuleUrl: (base: string, specifier: string): string =>
    new URL(specifier, base).href,
  loadModuleSync: (url: string, parameters: string) =>
    loadModuleSync(url, parameters),
  // No suite here fetches anything, and so registers no future; the
  // runtimes only need these to exist, because `lynx.fetchBundle` and the
  // `Future` class close over them as each evaluates.
  fetchResource: () => { throw new Error("no fetch in this suite"); },
  waitFuture: () => { throw new Error("no future in this suite"); },
  takeFuture: () => { throw new Error("no future in this suite"); },
  settleFuture: () => { throw new Error("no future in this suite"); },
}));

/** What the mock host serves a load from, by the URL it is asked for. */
const hostFiles = new Map<string, string>();
/** The bodies among them, as the namespace of the module each one is. */
const hostModules = new Map<string, object>();
/** Every load, as the URL and the parameter list it was asked for. */
const moduleLoads: [string, string][] = [];

function loadModuleSync(url: string, parameters: string) {
  moduleLoads.push([url, parameters]);
  // A body `PageSource` registered: an ES module the engine compiled, linked
  // and evaluated, answered as its namespace object.
  const namespace = hostModules.get(url);
  if (namespace !== undefined) {
    return { url, kind: "module" as const, value: namespace };
  }
  const text = hostFiles.get(url);
  if (text === undefined) throw new Error(`cannot load '${url}'`);
  if (url.split(/[?#]/)[0]!.endsWith(".json")) {
    return { url, kind: "json" as const, value: JSON.parse(text) };
  }
  return {
    url,
    kind: "commonjs" as const,
    value: new Function(...parameters.split(", "), text),
  };
}

/**
 * The members of the realm global the two runtimes reach, as this suite
 * installs them on Node's `globalThis`, plus the Lepus methods a test files
 * there for `callLepusMethod` to find.
 */
interface TestScope {
  processData: ((data: unknown, processor: string) => unknown) | undefined;
  renderPage: ((data: unknown, options: unknown) => unknown) | undefined;
  updatePage: ((data: unknown, options: unknown) => unknown) | undefined;
  removeComponents: (() => unknown) | undefined;
  updateGlobalProps: unknown;
  /** Absent in Node; the worker realm installs it, and the BTS test stands in. */
  reportError?: (error: unknown) => void;
  /** What a loaded Lepus chunk calls with the bindings it was given. */
  __bobcatChunkRan?: (...bindings: unknown[]) => void;
  postMessage(message: unknown): void;
  addEventListener(
    name: string,
    callback: (event: { data: unknown }) => void | Promise<void>,
  ): void;
  emptyLepusMethod?: () => undefined;
  slowLepusMethod?: () => Promise<unknown>;
  fastLepusMethod?: () => Promise<null>;
  nestedLepusMethod?: () => unknown;
  rejectedLepusMethod?: () => Promise<never>;
  'method; throw Error("name evaluated")'?: () => unknown;
  failedLepusMethod?: (data: unknown) => unknown;
  inspectLepusData?: (data: unknown) => unknown;
  functionLepusMethod?: (data: unknown) => unknown;
}

/** A message for the background realm, as the transport's own copy of it. */
interface Recorded {
  type?: unknown;
  method?: unknown;
  [field: string]: unknown;
}

const scope = globalThis as unknown as TestScope;
const originalPostMessage = scope.postMessage;
const originalAddEventListener = scope.addEventListener;
let mts: typeof mtsRuntime;
let bts: typeof btsRuntime.lynx;
let registerBundle: typeof btsRuntime.__BobcatRegisterBundle;
let receiveInBackground: (event: { data: unknown }) => void | Promise<void>;
const toBackground: Recorded[] = [];
const toMain: unknown[] = [];
const worker = Object.assign(new eventTarget.EventTarget(), {
  terminate: rstest.fn(),
  postMessage(message: unknown) {
    // `structuredClone` stands in for the host boundary's structured clone.
    toBackground.push(structuredClone(message) as Recorded);
  },
});

beforeAll(async () => {
  diagnostics = await import("../src/diagnostics.ts");
  sectionUrls = await import("../src/section-url.ts");
  futures = await import("../src/future.ts");
  bundleFetches = await import("../src/bundle-fetch.ts");
  moduleTable = await import("../src/lynx-modules.ts");
  animationFrames = await import("../src/animation-frame.ts");
  mts = await import("../src/main-thread-runtime.ts");
  mts.__BobcatInitEntry("https://example.test/page/main.js?version=2#entry");
  scope.emptyLepusMethod = () => undefined;
  scope.postMessage = (message: unknown) => {
    toMain.push(structuredClone(message));
  };
  scope.addEventListener = (name: string, callback) => {
    if (name === "message") receiveInBackground = callback;
  };
  const background = await import("../src/background-thread-runtime.ts");
  bts = background.lynx;
  registerBundle = background.__BobcatRegisterBundle;
});

afterAll(() => {
  delete scope.emptyLepusMethod;
  scope.postMessage = originalPostMessage;
  scope.addEventListener = originalAddEventListener;
});

function deliverToBackground() {
  return receiveInBackground({ data: toBackground.shift() });
}

async function deliverToMain() {
  worker.dispatchEvent({ type: "message", data: toMain.shift() });
  // Run the MTS await continuation that sends the reply.
  await Promise.resolve();
  await Promise.resolve();
}

describe("MTS/BTS lifecycle runtime", () => {
  it("answers nativeApp.loadScript out of the body it loads beside the bundle", () => {
    // All the boot script registers is where the container answered from.
    // Every path of it is a load beside that URL: a body `PageSource`
    // registered, whose module default-exported the `{init}` object a
    // `.lynx.bundle`'s expression evaluated to…
    registerBundle("https://cdn.test/app/x.web.bundle");
    hostModules.set("https://cdn.test/app/section.js",
      {default: {init: ({tt}: {tt: unknown}) => ({app: tt})}});
    // …or a path no container carried, which is a plain CommonJS file.
    hostFiles.set("https://cdn.test/app/external.js", "module.exports = {external: true};");

    const loaded = bts.getNativeApp().loadScript("/section.js");
    expect(loaded.init({tt: bts.getApp()}).app).toBe(bts.getApp());
    expect(bts.requireModule("/section.js").app).toBe(bts.getApp());
    expect(bts.requireModule("/external.js")).toEqual({external: true});
    // `loadScript` feeds no `requireModule` cache, so the body is asked for
    // twice — one URL, one module in the realm, evaluated once.
    expect(moduleLoads).toEqual([
      ["https://cdn.test/app/section.js", "module, exports"],
      ["https://cdn.test/app/section.js", "module, exports"],
      ["https://cdn.test/app/external.js", "module, exports"],
    ]);
  });

  it("resolves the card alias to stylesheet URLs and keeps opaque handles", () => {
    expect(mts.__Card__).toBe("https://example.test/page/main.js?version=2#entry");
    const first = mts.__LoadStyleSheet('CSS', '__Card__');
    const second = mts.__LoadStyleSheet('CSS', mts.__Card__);
    expect(preloadStyleSheet.mock.calls).toEqual([
      ['https://example.test/page/main.js/index.css?version=2#entry'],
      ['https://example.test/page/main.js/index.css?version=2#entry'],
    ]);
    expect(first).not.toBe(second);
    expect(mts.__AdoptStyleSheet(first)).toBeNull();
    expect(mts.__AdoptStyleSheet(first)).toBeNull();
    expect(adoptStyleSheet.mock.calls).toEqual([
      ['https://example.test/page/main.js/index.css?version=2#entry'],
      ['https://example.test/page/main.js/index.css?version=2#entry'],
    ]);
    // The handle is only as good as the URL it names: no realm state stands
    // behind it any more.
    expect(first).toEqual({url: 'https://example.test/page/main.js/index.css?version=2#entry'});
    expect(() => mts.__AdoptStyleSheet({} as {url: string})).toThrow();
    expect(() => Reflect.apply(mts.__AdoptStyleSheet, undefined, [null])).toThrow();
    expect(() => Reflect.apply(mts.__LoadStyleSheet, undefined, ['CSS'])).toThrow();
  });

  it("passes external and escaped stylesheet URLs to the same loader", () => {
    preloadStyleSheet.mockClear();
    mts.__LoadStyleSheet('CSS', 'https://cdn.test/component.bundle?rev=3');
    mts.__LoadStyleSheet('A &/中', './component.bundle#entry');
    expect(preloadStyleSheet.mock.calls).toEqual([
      ['https://cdn.test/component.bundle/index.css?rev=3'],
      ['./component.bundle/A%20%26%2F%E4%B8%AD/index.css#entry'],
    ]);
  });

  it("throws a load failure from adopt in the same call", () => {
    const handle = mts.__LoadStyleSheet('CSS', '__Card__');
    adoptStyleSheet.mockImplementationOnce(() => { throw Error('CSS unavailable'); });
    expect(() => mts.__AdoptStyleSheet(handle)).toThrow('CSS unavailable');
  });

  it("loads a named Lepus chunk through the host and runs it on every call", () => {
    moduleLoads.length = 0;
    reportedErrors.mockClear();
    // The chunk URL is `PageSource`'s own `named_chunk_url`: the entry path,
    // the percent-encoded name as a `.js` file, the `?#` suffix kept.
    const worklet = "https://example.test/page/main.js/worklet.js?version=2#entry";
    const escaped =
      "https://example.test/page/main.js/A%20%26%2F%E4%B8%AD.js?version=2#entry";
    const ran: unknown[][] = [];
    scope.__bobcatChunkRan = (...bindings: unknown[]) => { ran.push(bindings); };
    // A chunk's bindings are the parameters of the body the host compiled,
    // and its `var` is local to the call.
    hostFiles.set(worklet,
      "__bobcatChunkRan(__Card__, lynx, __LoadStyleSheet, __BobcatQueryNodes);"
      + " var chunkLocal = 1;");
    hostFiles.set(escaped, "__bobcatChunkRan();");

    expect(mts.__LoadLepusChunk("worklet", {})).toBe(true);
    expect(mts.__LoadLepusChunk("worklet", {dynamicComponentEntry: "__Card__"})).toBe(true);
    expect(mts.__LoadLepusChunk("A &/中", {dynamicComponentEntry: mts.__Card__})).toBe(true);
    // One load and one run per call: nothing caches a chunk.
    expect(moduleLoads.map(([url]) => url)).toEqual([worklet, worklet, escaped]);
    expect(ran).toHaveLength(3);
    expect(ran[0]).toEqual([mts.__Card__, mts.lynx, mts.__LoadStyleSheet, queryNodes]);
    expect("chunkLocal" in scope).toBe(false);
    // Every binding the entry preamble imports is a parameter, PAPI included.
    const parameters = moduleLoads[0]![1].split(", ");
    expect(parameters).toEqual(expect.arrayContaining([
      "__BobcatQueryNodes", "__Card__", "lynx", "console", "SystemInfo",
      "__globalProps", "NativeModules", "_AddEventListener", "_ReportError",
      "_SetSourceMapRelease", "__OnLifecycleEvent", "__LoadLepusChunk",
      "__LoadStyleSheet", "__AdoptStyleSheet",
    ]));
    expect(new Set(parameters).size).toBe(parameters.length);
    expect(reportedErrors).not.toHaveBeenCalled();

    // A chunk of another component is neither loaded nor run.
    moduleLoads.length = 0;
    expect(mts.__LoadLepusChunk("worklet", {dynamicComponentEntry: "absent"})).toBe(false);
    expect(moduleLoads).toEqual([]);
  });

  it("answers a chunk the host cannot load with false, and a chunk that fails with true", () => {
    moduleLoads.length = 0;
    reportedErrors.mockClear();
    // No such chunk: the load throws an ordinary Error, and nothing is
    // reported — this page simply does not carry that name.
    expect(mts.__LoadLepusChunk("missing", {})).toBe(false);
    expect(moduleLoads.map(([url]) => url))
      .toEqual(["https://example.test/page/main.js/missing.js?version=2#entry"]);
    expect(reportedErrors).not.toHaveBeenCalled();

    // Found, and its body threw: native answers true for a chunk it found,
    // and the exception is reported.
    hostFiles.set("https://example.test/page/main.js/throws.js?version=2#entry",
      "throw Error('chunk failed');");
    expect(mts.__LoadLepusChunk("throws", {})).toBe(true);
    expect(reportedErrors).toHaveBeenCalledTimes(1);
    expect(reportedErrors.mock.calls[0]![1]).toContain("chunk failed");

    // Found, and it does not compile: the host answers a SyntaxError, which
    // is a chunk that exists too.
    reportedErrors.mockClear();
    hostFiles.set("https://example.test/page/main.js/broken.js?version=2#entry", "(");
    expect(mts.__LoadLepusChunk("broken", {})).toBe(true);
    expect(reportedErrors).toHaveBeenCalledTimes(1);
    expect(reportedErrors.mock.calls[0]![0]).toBe("error");

    expect(() => Reflect.apply(mts.__LoadLepusChunk, undefined, ["worklet"])).toThrow(TypeError);
    expect(() => Reflect.apply(mts.__LoadLepusChunk, undefined, ["worklet", null]))
      .toThrow(TypeError);
    expect(() => Reflect.apply(mts.__LoadLepusChunk, undefined, [1, {}])).toThrow(TypeError);
  });

  it("reports a microtask throw before the next job and ignores callback return values", async () => {
    // The worker realm's global `reportError`, which a throw goes to as an
    // uncaught exception does; Node has none, so the test stands one in.
    const reportError = rstest.fn();
    scope.reportError = reportError;
    reportedErrors.mockClear();
    const then = rstest.fn();
    const before = toMain.length;
    const failure = Error("microtask failure");
    bts.queueMicrotask(() => ({then}));
    bts.queueMicrotask(() => { throw failure; });
    bts.queueMicrotask(() => bts.reportError("after microtask"));
    await Promise.resolve();
    expect(then).not.toHaveBeenCalled();
    expect(reportError).toHaveBeenCalledExactlyOnceWith(failure);
    expect(reportedErrors.mock.calls).toEqual([["error", "after microtask"]]);
    expect(toMain).toHaveLength(before);
    delete scope.reportError;
  });

  it("asks the host for a frame through requestScriptFrame from both runtimes' lynx", async () => {
    // What each frame does is `bobcat:animation-frame`'s, and
    // animation-frame.test.ts pins it; this pins that both runtimes hand
    // `lynx.requestAnimationFrame` to it.
    for (const runtime of [mts, await import("../src/background-thread-runtime.ts")]) {
      expect(runtime.lynx.requestAnimationFrame).toBe(animationFrames.requestAnimationFrame);
      expect(runtime.lynx.cancelAnimationFrame).toBe(animationFrames.cancelAnimationFrame);
      requestScriptFrame.mockClear();
      const id = runtime.lynx.requestAnimationFrame(() => undefined);
      runtime.lynx.cancelAnimationFrame(id);
      expect(requestScriptFrame.mock.calls).toEqual([[true], [false]]);
    }
  });

  it("queues Context and publish calls together, then replays each late publish hook", () => {
    const seen: unknown[] = [];
    bts.getCoreContext().addEventListener("custom", (event: { data: unknown }) => {
      seen.push(["context", event.data]);
    });
    const contextEvent = {
      type: "custom", data: 0, bobcat: "runtime", method: "callDestroyLifetimeFun",
    };
    mts.__BobcatPublishEvent(undefined, "first", { value: 1 });
    mts.lynx.getJSContext().dispatchEvent(contextEvent);
    mts.__BobcatPublishEvent("component", "second", { value: 3 });
    contextEvent.data = 2;
    mts.__BobcatConnectBackground(worker as unknown as Worker, {seed: 1});
    expect(toBackground.shift()).toMatchObject({bobcat: "runtime", method: "initialize", updateData: {seed: 1}});
    expect(toBackground.map((message) => message.method ?? message.type)).toEqual([
      "publishEvent", "custom", "publicComponentEvent",
    ]);
    expect(toBackground[1]).toEqual({ type: "custom", data: 0, origin: "CoreContext" });
    deliverToBackground();
    deliverToBackground();
    deliverToBackground();
    expect(seen).toEqual([["context", 0]]);
    const app = bts.getApp();
    app.publicComponentEvent = function (...args) {
      expect(this).toBe(app);
      seen.push(args);
    };
    app.publishEvent = function (...args) {
      expect(this).toBe(app);
      seen.push(args);
    };
    expect(seen).toEqual([
      ["context", 0], ["component", "second", { value: 3 }], ["first", { value: 1 }],
    ]);
    app.publishEvent = function (...args) { seen.push(["replacement", ...args]); };
    mts.__BobcatPublishEvent(0, "third", { value: 4 });
    deliverToBackground();
    expect(seen.at(-1)).toEqual(["replacement", "third", { value: 4 }]);
    expect(bts.getApp()).toBe(app);
    expect(bts.getNativeApp()).toBe(bts.getNativeApp());
  });

  it("snapshots node requests and removes query callbacks even when they throw", async () => {
    queryNodes.mockImplementation(() => ({data:{id:'target'}, status:{code:0,data:'success'}}));
    const callback = rstest.fn(() => { throw Error('query callback failed'); });
    bts.createSelectorQuery().select('#target').fields({id:true}, callback)?.exec();
    await deliverToMain();
    const reply = toBackground[0];
    expect(() => deliverToBackground()).toThrow('query callback failed');
    receiveInBackground({data:reply});
    expect(callback).toHaveBeenCalledTimes(1);
    const next = rstest.fn();
    bts.createSelectorQuery().select('#target').fields({id:true}, next)?.exec();
    await deliverToMain(); deliverToBackground();
    expect(next).toHaveBeenCalledWith({id:'target'}, {code:0,data:'success'});
    const before = toMain.length;
    expect(() => bts.createSelectorQuery().select(7 as unknown as string).fields({id:true})?.exec()).toThrow('identifier');
    expect(toMain).toHaveLength(before);
  });

  it("answers a query whose result the transport refuses with status 1", async () => {
    // A function, not a BigInt: the transport carries a BigInt, so what a
    // reply can still fail on is a value the serializer has no encoding for.
    queryNodes.mockImplementationOnce(() => ({data:{attribute:{value:() => 7}}, status:{code:0,data:'success'}}));
    const callback = rstest.fn();
    bts.createSelectorQuery().select('#target').fields({attribute:true}, callback)?.exec();
    await deliverToMain(); deliverToBackground();
    expect(callback).toHaveBeenCalledWith(null, {code:1,data:expect.stringMatching(/clon|serializ|unsupported/i)});
  });


  it("keeps extra Context properties separate from runtime messages in both directions", async () => {
    const seen: unknown[] = [];
    mts.lynx.getJSContext().addEventListener("protocol-like", (event: unknown) => {
      seen.push(["MTS", event]);
    });
    bts.getCoreContext().addEventListener("protocol-like", (event: unknown) => {
      seen.push(["BTS", event]);
    });
    const event = {
      type: "protocol-like", data: 7, bobcat: "runtime", method: "callDestroyLifetimeFun",
    };
    mts.lynx.getJSContext().dispatchEvent(event);
    expect(toBackground[0]).toEqual({ type: "protocol-like", data: 7, origin: "CoreContext" });
    deliverToBackground();
    bts.getCoreContext().dispatchEvent(event);
    expect(toMain[0]).toEqual({ type: "protocol-like", data: 7, origin: "JSContext" });
    await deliverToMain();
    expect(seen).toEqual([
      ["BTS", { type: "protocol-like", data: 7, origin: "CoreContext" }],
      ["MTS", { type: "protocol-like", data: 7, origin: "JSContext" }],
    ]);
  });

  it("reads the current global property and schedules both reply and callback as Promise jobs", async () => {
    const name = 'method; throw Error("name evaluated")';
    const callback = rstest.fn();
    scope[name] = () => 'before delivery';
    try {
      bts.getNativeApp().callLepusMethod(name, {}, callback);
      scope[name] = function () {
        expect(this).toBe(scope);
        return 'at delivery';
      };
      worker.dispatchEvent({type: "message", data: toMain.shift()});
      expect(toBackground).toHaveLength(0);
      await Promise.resolve();
      expect(toBackground[0]).toMatchObject({method: "callLepusMethodResult", result: 'at delivery'});
      const delivery = deliverToBackground();
      expect(callback).not.toHaveBeenCalled();
      await delivery;
      expect(callback).toHaveBeenCalledWith('at delivery');
    } finally { delete scope[name]; }
  });

  it("awaits returned Promises without blocking later calls or mixing their callbacks", async () => {
    let resolveSlow: (value: unknown) => void = () => {};
    const slow = rstest.fn();
    const fast = rstest.fn();
    scope.slowLepusMethod = () => new Promise(resolve => { resolveSlow = resolve; });
    scope.fastLepusMethod = async () => null;
    try {
      bts.getNativeApp().callLepusMethod("slowLepusMethod", {}, slow);
      await deliverToMain();
      expect(toBackground).toHaveLength(0);
      bts.getNativeApp().callLepusMethod("fastLepusMethod", {}, fast);
      await deliverToMain(); await deliverToBackground();
      expect(fast.mock.calls).toEqual([[null]]);
      expect(slow).not.toHaveBeenCalled();
      resolveSlow({answer: 42});
      await Promise.resolve();
      await deliverToBackground();
      expect(slow).toHaveBeenCalledWith({answer: 42});
    } finally {
      delete scope.slowLepusMethod;
      delete scope.fastLepusMethod;
    }
  });

  it("snapshots at the await continuation rather than draining every nested job", async () => {
    const callback = rstest.fn();
    const result = {value: 0};
    scope.nestedLepusMethod = () => {
      Promise.resolve().then(() => Promise.resolve().then(() => result.value++));
      return result;
    };
    try {
      bts.getNativeApp().callLepusMethod("nestedLepusMethod", {}, callback);
      await deliverToMain(); await deliverToBackground();
      expect(result.value).toBe(1);
      expect(callback).toHaveBeenCalledWith({value: 0});
    } finally { delete scope.nestedLepusMethod; }
  });

  it("reports throws and rejected results without invoking success callbacks", async () => {
    const callback = rstest.fn();
    scope.failedLepusMethod = () => { throw new TypeError("method failed"); };
    scope.rejectedLepusMethod = async () => { throw new Error("method rejected"); };
    try {
      for (const name of ["failedLepusMethod", "rejectedLepusMethod"]) {
        bts.getNativeApp().callLepusMethod(name, {}, callback);
        await deliverToMain();
        const reply = toBackground[0];
        await expect(deliverToBackground()).rejects.toThrow(/method (failed|rejected)/);
        await receiveInBackground({data: {...reply, error: undefined, result: "duplicate"}});
        expect(callback).not.toHaveBeenCalled();
      }
      bts.getNativeApp().callLepusMethod("rejectedLepusMethod", {});
      await deliverToMain();
      await expect(deliverToBackground()).rejects.toThrow("method rejected");
      bts.getNativeApp().callLepusMethod("absentLepusMethod", {}, callback);
      await deliverToMain(); await deliverToBackground();
      expect(callback.mock.calls).toEqual([[undefined]]);
    } finally {
      delete scope.failedLepusMethod;
      delete scope.rejectedLepusMethod;
    }
  });

  it("releases callback IDs before a throwing callback or a duplicate reply", async () => {
    const callback = rstest.fn(() => { throw new Error("callback failed"); });
    bts.getNativeApp().callLepusMethod("emptyLepusMethod", {}, callback);
    await deliverToMain();
    const reply = toBackground[0];
    const delivery = deliverToBackground();
    const duplicate = receiveInBackground({data: reply});
    expect(callback).not.toHaveBeenCalled();
    await expect(delivery).rejects.toThrow("callback failed");
    await duplicate;
    expect(callback).toHaveBeenCalledTimes(1);
    await receiveInBackground({data: reply});
    expect(callback).toHaveBeenCalledTimes(1);
  });

  it("ignores primitive RPC arguments and keeps the Worker structured-clone value semantics", async () => {
    const callback = rstest.fn();
    scope.inspectLepusData = (data: unknown) => data;
    try {
      // Native BTS filters a non-object argument before any send, which is
      // what keeps a BigInt, a Symbol or a function from reaching the
      // transport as the whole payload.
      for (const data of [undefined, null, false, 42, "text", 1n, Symbol(), () => {}]) {
        bts.getNativeApp().callLepusMethod("inspectLepusData", data, callback);
      }
      expect(toMain).toEqual([]);
      // Inside an object, the transport preserves what JSON could not: an
      // `undefined`-valued member, negative zero, the nonfinite numbers,
      // array holes, a BigInt, and a cycle.
      const data = {
        missing: undefined,
        nil: null as null | string,
        negativeZero: -0,
        nan: NaN,
        infinity: Infinity,
        big: 9007199254740993n,
        array: [undefined, null],
        object: { bobcat: "value", value: ["undefined"] },
        self: undefined as unknown,
      };
      data.self = data;
      bts.getNativeApp().callLepusMethod("inspectLepusData", data, callback);
      // Copied at send time, so this never reaches the other side.
      data.nil = "changed after send";
      await deliverToMain(); await deliverToBackground();
      const seen = callback.mock.calls[0]?.[0] as Record<string, unknown>;
      expect(callback.mock.calls).toHaveLength(1);
      expect(Object.hasOwn(seen, "missing")).toBe(true);
      expect(seen["missing"]).toBeUndefined();
      expect(seen["nil"]).toBeNull();
      expect(Object.is(seen["negativeZero"], -0)).toBe(true);
      expect(Number.isNaN(seen["nan"])).toBe(true);
      expect(seen["infinity"]).toBe(Infinity);
      expect(seen["big"]).toBe(9007199254740993n);
      expect(seen["array"]).toEqual([undefined, null]);
      expect(seen["object"]).toEqual({ bobcat: "value", value: ["undefined"] });
      expect(seen["self"]).toBe(seen);
    } finally { delete scope.inspectLepusData; }
  });

  it("releases callbacks on failed sends and reports refused result values", async () => {
    const callback = rstest.fn();
    // A function is what the transport refuses; a BigInt now crosses.
    expect(() => bts.getNativeApp().callLepusMethod(
      "emptyLepusMethod", { value: () => undefined }, callback)).toThrow();
    const postMessage = scope.postMessage;
    let failedId: number | undefined;
    scope.postMessage = (message: unknown) => {
      failedId = (message as { id?: number }).id;
      throw new Error("send failed");
    };
    try {
      expect(() => bts.getNativeApp().callLepusMethod("emptyLepusMethod", {}, callback)).toThrow("send failed");
      expect(failedId).toBeDefined();
      await receiveInBackground({data: {
        bobcat: "runtime", method: "callLepusMethodResult", id: failedId, result: 1,
      }});
      expect(callback).not.toHaveBeenCalled();
    } finally { scope.postMessage = postMessage; }
    scope.functionLepusMethod = () => () => undefined;
    try {
      bts.getNativeApp().callLepusMethod("functionLepusMethod", {}, callback);
      await deliverToMain();
      await expect(deliverToBackground()).rejects.toThrow();
      expect(callback).not.toHaveBeenCalled();
    } finally { delete scope.functionLepusMethod; }
  });



});

describe("runtime events and diagnostics", () => {
  it("reports engine listener errors through dispatchEvent and preserves the event walk", async () => {
    const engine = mts.lynx.getEngine();
    const event = {type: "engine-error-test", data: 1, defaultPrevented: true};
    const order: string[] = [];
    const first = rstest.fn(function(this: unknown, received: unknown) {
      expect(this).toBe(engine);
      expect(received).toBe(event);
      order.push("first");
      Promise.resolve().then(() => { order.push("job"); });
      throw Error("engine listener failed");
    });
    const second = rstest.fn((received: unknown) => {
      expect(received).toBe(event);
      order.push("second");
    });
    reportedErrors.mockClear();
    engine.addEventListener(event.type, first, {once: true});
    engine.addEventListener(event.type, second);
    try {
      expect(engine.dispatchEvent(event)).toBe(false);
      expect(order).toEqual(["first", "second"]);
      expect(reportedErrors).toHaveBeenCalledExactlyOnceWith("error", expect.stringContaining("engine listener failed"));
      engine.removeEventListener(event.type, second);
      expect(engine.dispatchEvent({...event, defaultPrevented: false})).toBe(true);
      expect(first).toHaveBeenCalledTimes(1);
      expect(second).toHaveBeenCalledTimes(1);
      await Promise.resolve();
      expect(order).toEqual(["first", "second", "job"]);
    } finally {
      engine.removeEventListener(event.type, first);
      engine.removeEventListener(event.type, second);
      reportedErrors.mockClear();
    }
  });

  it("isolates the listeners of a Context event arriving from the BTS Worker", () => {
    const jsContext = mts.lynx.getJSContext();
    const order: string[] = [];
    const first = () => { order.push("first"); throw Error("context listener failed"); };
    const second = () => { order.push("second"); };
    reportedErrors.mockClear();
    jsContext.addEventListener("isolated", first);
    jsContext.addEventListener("isolated", second);
    try {
      // The host entry that delivers the Worker message must see nothing of
      // the throw, and the listener behind it must still run.
      expect(() => worker.dispatchEvent({
        type: "message", data: {type: "isolated", data: 1, origin: "CoreContext"},
      })).not.toThrow();
      expect(order).toEqual(["first", "second"]);
      expect(reportedErrors).toHaveBeenCalledExactlyOnceWith(
        "error", expect.stringContaining("context listener failed"));
    } finally {
      jsContext.removeEventListener("isolated", first);
      jsContext.removeEventListener("isolated", second);
      reportedErrors.mockClear();
    }
  });

  it("reports a throwing BTS Context listener in the worker realm and runs the rest", () => {
    // worker-runtime.ts installs the worker realm's reporter, and this suite
    // does not load it (`bobcat:worker` is mocked), so stand in for it
    // with the same function it would install: the realm's `reportError`.
    const reportError = rstest.fn();
    scope.reportError = reportError;
    eventTarget.installExceptionReporter(reportError);
    const coreContext = bts.getCoreContext();
    const order: string[] = [];
    const failure = Error("BTS context listener failed");
    const first = () => { order.push("first"); throw failure; };
    const second = () => { order.push("second"); };
    coreContext.addEventListener("bts-isolated", first);
    coreContext.addEventListener("bts-isolated", second);
    try {
      receiveInBackground({data: {type: "bts-isolated", data: 1, origin: "JSContext"}});
      expect(order).toEqual(["first", "second"]);
      expect(reportError).toHaveBeenCalledExactlyOnceWith(failure);
    } finally {
      coreContext.removeEventListener("bts-isolated", first);
      coreContext.removeEventListener("bts-isolated", second);
      eventTarget.installExceptionReporter(mts._ReportError);
      delete scope.reportError;
    }
  });

  it("uses full host props, current MTS hooks and native engine-event precedence", () => {
    const old=scope.updateGlobalProps;
    const initial={initData:mts.lynx.__initData,globalProps:mts.lynx.__globalProps,systemInfo:mts.SystemInfo};
    mts.__BobcatInitializeMTS({...initial,globalProps:{keep:1,nested:{value:2}}});
    const before=mts.lynx.__globalProps;
    (before['nested'] as {value: number}).value=99;
    const apply=(data: unknown)=>mts.__BobcatUpdateGlobalProps(JSON.stringify(data));
    const listener=rstest.fn(event=>{
      expect(event).toEqual({type:'__UpdateGlobalProps',data:[mts.lynx.__globalProps]});
      expect(toBackground.at(-1)).toEqual({bobcat:'runtime',method:'updateGlobalProps',args:[mts.lynx.__globalProps]});
      throw Error('global props hook failed');
    });
    const hook=rstest.fn();
    try {
      scope.updateGlobalProps=hook;
      mts.lynx.getEngine().addEventListener('__UpdateGlobalProps',listener);
      apply(JSON.parse('{"seed":3,"__proto__":{"own":true},"a.b":4}'));
      expect(hook).not.toHaveBeenCalled();
      expect(listener).toHaveBeenCalledTimes(1);
      expect(reportedErrors).toHaveBeenLastCalledWith('error',expect.stringContaining('global props hook failed'));
      expect((mts.lynx.__globalProps['nested'] as {value: number}).value).toBe(2);
      expect(before).toEqual({keep:1,nested:{value:99}});
      expect(Object.hasOwn(mts.lynx.__globalProps,'__proto__')).toBe(true);
      expect(Object.getPrototypeOf(mts.lynx.__globalProps)).toBe(Object.prototype);
      expect(mts.lynx.__globalProps['a.b']).toBe(4);
      toBackground.shift();
      mts.lynx.getEngine().removeEventListener('__UpdateGlobalProps',listener);
      apply({keep:null});
      expect(hook).toHaveBeenCalledWith(mts.lynx.__globalProps);
      expect(mts.lynx.__globalProps).toMatchObject({seed:3,keep:null});
      toBackground.shift();
      scope.updateGlobalProps=3;
      expect(()=>apply({seed:4})).not.toThrow();
      toBackground.shift();
    } finally {
      mts.lynx.getEngine().removeEventListener('__UpdateGlobalProps',listener);
      scope.updateGlobalProps=old;
      mts.__BobcatInitializeMTS(initial);
    }
  });

  it("processes reload data and enqueues BTS reload before the new MTS first-screen event", () => {
    const oldProcess=scope.processData, oldRemove=scope.removeComponents, oldUpdate=scope.updatePage;
    const app=bts.getApp();
    const oldReload=app.onAppReload, oldLifecycle=app.OnLifecycleEvent;
    const order: unknown[]=[];
    try {
      scope.processData=(data, processor) => {
        order.push(['process',processor]);
        return {seed:(data as {seed: number}).seed+1};
      };
      scope.removeComponents=() => { order.push('remove'); };
      scope.updatePage=(data, options) => {
        order.push(['main',data,options]);
        expect(toBackground.at(-1)).toEqual({bobcat:'runtime',method:'onAppReload',args:[{seed:3},{processorName:''}]});
        mts.__OnLifecycleEvent(['first-screen', {seed:(data as {seed: number}).seed}]);
      };
      app.onAppReload=function(...args) { expect(this).toBe(app); order.push(['background',...args]); };
      app.OnLifecycleEvent=function(...args) { order.push(['lifecycle',...args]); };
      mts.__BobcatReload(JSON.stringify({seed:2}),'');
      deliverToBackground();
      deliverToBackground();
      expect(order).toEqual([
        ['process',''], 'remove',
        ['main',{seed:3},{resetPageData:false,reloadFromJS:false,reloadTemplate:true,nativeUpdateDataOrder:0}],
        ['background',{seed:3},{processorName:''}],
        ['lifecycle',['first-screen',{seed:3}]],
      ]);
      scope.updatePage=() => {};
      for (const ignored of [null, [], 7, undefined]) {
        scope.processData=() => ignored;
        mts.__BobcatReload(JSON.stringify({seed:4}),'');
        expect(toBackground.shift()).toEqual({bobcat:'runtime',method:'onAppReload',args:[{seed:4},{processorName:''}]});
      }
      const failures=reportedErrors.mock.calls.length;
      scope.processData=() => {throw Error('processor failed');};
      scope.removeComponents=() => {throw Error('removal failed');};
      scope.updatePage=() => {throw Error('render failed');};
      mts.__BobcatReload(JSON.stringify({seed:5}),'');
      expect(toBackground.shift()).toEqual({bobcat:'runtime',method:'onAppReload',args:[{seed:5},{processorName:''}]});
      expect(reportedErrors.mock.calls.slice(failures).map(call=>call[1])).toEqual([
        expect.stringContaining('processor failed'),expect.stringContaining('removal failed'),expect.stringContaining('render failed'),
      ]);
      const engine=mts.lynx.getEngine(), remove=rstest.fn(), update=rstest.fn();
      engine.addEventListener('__RemoveComponents',remove);
      engine.addEventListener('__UpdatePage',update);
      try {
        scope.processData=() => ({seed:6});
        const reports=reportedErrors.mock.calls.length;
        mts.__BobcatReload(JSON.stringify({seed:5}),'');
        expect(reportedErrors.mock.calls).toHaveLength(reports);
        expect(remove).toHaveBeenCalledWith({type:'__RemoveComponents',data:[]});
        expect(update).toHaveBeenCalledWith({type:'__UpdatePage',data:[{seed:6},expect.objectContaining({reloadTemplate:true})]});
        expect(toBackground.shift()).toEqual({bobcat:'runtime',method:'onAppReload',args:[{seed:6},{processorName:''}]});
      } finally {
        engine.removeEventListener('__RemoveComponents',remove);
        engine.removeEventListener('__UpdatePage',update);
      }
    } finally {
      scope.processData=oldProcess; scope.removeComponents=oldRemove; scope.updatePage=oldUpdate;
      if (oldReload) app.onAppReload=oldReload; else delete app.onAppReload;
      if (oldLifecycle) app.OnLifecycleEvent=oldLifecycle; else delete app.OnLifecycleEvent;
    }
  });

  it("forwards processed update/reset data after MTS delivery, including native fallbacks and reported failures", () => {
    const oldProcess=scope.processData, oldUpdate=scope.updatePage;
    const app=bts.getApp(), oldHook=app.updateCardData;
    const update=rstest.fn();
    const hook=rstest.fn();
    const engine=mts.lynx.getEngine();
    engine.addEventListener('__UpdatePage',update);
    try {
      app.updateCardData=hook;
      scope.updatePage=() => {throw Error('engine listener must take precedence');};
      const processed=Object.fromEntries([
        ['count',42], ['missing',undefined], ['nan',NaN], ['negativeZero',-0],
        ['__proto__',{own:true}], ['infinity',Infinity],
      ]);
      const process=rstest.fn(() => processed);
      scope.processData=process;
      for (const type of [0,1]) {
        mts.__BobcatUpdateData(JSON.stringify({raw:7}),'',type === 1);
        expect(process).toHaveBeenLastCalledWith({raw:7},'');
        expect(update).toHaveBeenLastCalledWith({type:'__UpdatePage',data:[processed,{resetPageData:type===1,reloadFromJS:false,reloadTemplate:false,nativeUpdateDataOrder:0}]});
        expect(hook).toHaveBeenCalledTimes(type);
        deliverToBackground();
        expect(hook).toHaveBeenLastCalledWith(structuredClone(processed),{type,processorName:''});
        const received=hook.mock.calls.at(-1)?.[0];
        // The transport keeps an `undefined`-valued member, where JSON dropped it.
        expect(Object.hasOwn(received,'missing')).toBe(true);
        expect(received.missing).toBeUndefined();
        expect(Number.isNaN(received.nan)).toBe(true);
        expect(received.infinity).toBe(Infinity);
        expect(Object.is(received.negativeZero,-0)).toBe(true);
        expect(Object.hasOwn(received,'__proto__')).toBe(true);
        expect(received.own).toBeUndefined();
      }
      engine.removeEventListener('__UpdatePage',update);
      scope.updatePage=rstest.fn();
      for (const value of [undefined,null,[],false,7,'wrong',()=>({})]) {
        scope.processData=()=>value;
        mts.__BobcatUpdateData(JSON.stringify({raw:8}),'',false);
        expect(scope.updatePage).toHaveBeenLastCalledWith({raw:8},expect.objectContaining({resetPageData:false}));
        deliverToBackground();
        expect(hook).toHaveBeenLastCalledWith({raw:8},{type:0,processorName:''});
      }
      scope.processData=()=>{throw Error('update processor failed');};
      scope.updatePage=()=>{throw Error('update renderer failed');};
      const errors=reportedErrors.mock.calls.length;
      mts.__BobcatUpdateData(JSON.stringify({raw:9}),'',true);
      deliverToBackground();
      expect(hook).toHaveBeenLastCalledWith({raw:9},{type:1,processorName:''});
      expect(reportedErrors.mock.calls.slice(errors).map(call=>call[1])).toEqual([
        expect.stringContaining('update processor failed'),expect.stringContaining('update renderer failed'),
      ]);
    } finally {
      engine.removeEventListener('__UpdatePage',update);
      scope.processData=oldProcess; scope.updatePage=oldUpdate;
      if (oldHook) app.updateCardData=oldHook; else delete app.updateCardData;
    }
  });

  it("rejects malformed host JSON before invoking hooks or posting Worker messages", () => {
    const oldProcess=scope.processData, oldUpdate=scope.updatePage, oldRemove=scope.removeComponents;
    const hook=rstest.fn(), props=mts.lynx.__globalProps;
    const queued=toBackground.length;
    scope.processData=hook; scope.updatePage=hook; scope.removeComponents=hook;
    try {
      expect(() => mts.__BobcatUpdateData('{', '', false)).toThrow(SyntaxError);
      expect(() => mts.__BobcatUpdateData('{', '', true)).toThrow(SyntaxError);
      expect(() => mts.__BobcatReload('{', '')).toThrow(SyntaxError);
      expect(() => mts.__BobcatUpdateGlobalProps('{')).toThrow(SyntaxError);
      expect(() => mts.__BobcatSendGlobalEvent('event', '[')).toThrow(SyntaxError);
      expect(hook).not.toHaveBeenCalled();
      expect(mts.lynx.__globalProps).toBe(props);
      expect(toBackground).toHaveLength(queued);
    } finally {
      scope.processData=oldProcess; scope.updatePage=oldUpdate; scope.removeComponents=oldRemove;
    }
  });

  it("selects named MTS processors and preserves raw data/name only in JS processor mode", () => {
    const oldProcess=scope.processData, oldRender=scope.renderPage, oldUpdate=scope.updatePage, oldRemove=scope.removeComponents;
    const initial={initData:mts.lynx.__initData,globalProps:mts.lynx.__globalProps,systemInfo:mts.SystemInfo};
    const engine=mts.lynx.getEngine(), render=rstest.fn();
    engine.addEventListener('__RenderPage',render);
    try {
      for (const onJS of [false,true]) {
        mts.__BobcatInitializeMTS({...initial,processorName:'initial',enableJSDataProcessor:onJS});
        const process=rstest.fn((data,name)=>({value:data.raw+1,name}));
        scope.processData=process;
        scope.renderPage=()=>{throw Error('legacy renderer must not run beside engine listener');};
        scope.updatePage=rstest.fn();
        scope.removeComponents=()=>{};
        const data=mts.__BobcatProcessInitData({raw:3});
        mts.__BobcatRenderPage(data);
        expect(data).toEqual(onJS?{raw:3}:{value:4,name:'initial'});
        expect(render).toHaveBeenLastCalledWith({type:'__RenderPage',data:[data,{preLoadTemplate:false,...(onJS?{processorName:'initial'}:{})}]});
        for (const type of [0,1]) {
          mts.__BobcatUpdateData(JSON.stringify({raw:5}),'next',type === 1);
          const data=onJS?{raw:5}:{value:6,name:'next'};
          expect(scope.updatePage).toHaveBeenLastCalledWith(data,{resetPageData:type===1,reloadFromJS:false,reloadTemplate:false,nativeUpdateDataOrder:0,...(onJS?{processorName:'next'}:{})});
          expect(toBackground.shift()).toEqual({bobcat:'runtime',method:'updateCardData',args:[data,{type,processorName:onJS?'next':''}]});
        }
        mts.__BobcatReload(JSON.stringify({raw:7}),'reload');
        expect(toBackground.shift()).toEqual({bobcat:'runtime',method:'onAppReload',args:[onJS?{raw:7}:{value:8,name:'reload'},{processorName:onJS?'reload':''}]});
        expect(process.mock.calls.map(call=>call[1])).toEqual(onJS?[]:['initial','next','next','reload']);
      }
    } finally {
      mts.__BobcatInitializeMTS(initial);
      scope.processData=oldProcess; scope.renderPage=oldRender; scope.updatePage=oldUpdate; scope.removeComponents=oldRemove;
      engine.removeEventListener('__RenderPage',render);
    }
  });

  it("uses native reload argument coercion and snapshots object data at the call", () => {
    const start=toMain.length;
    for (const value of [undefined,null,7,'data',false,Symbol('ignored'),1n]) {
      expect(bts.reload(value,17)).toBeUndefined();
    }
    const data={seed:2,nested:{keep:undefined}};
    bts.reload(data);
    data.seed=9;
    // A cycle crosses now, so it rides the successful send rather than the
    // refusal below.
    const cycle: {self?: unknown}={}; cycle.self=cycle;
    bts.reload(cycle);
    const sent=toMain.splice(start) as {data: {self?: unknown}}[];
    expect(sent.slice(0,8)).toEqual([
      ...Array(7).fill({bobcat:'runtime',method:'reloadFromJS',data:{}}),
      {bobcat:'runtime',method:'reloadFromJS',data:{seed:2,nested:{}}},
    ]);
    expect(Object.hasOwn(sent[7]!.data as object,'nested')).toBe(true);
    expect(sent[8]!.data.self).toBe(sent[8]!.data);
    const callback=rstest.fn();
    expect(bts.reload([],callback)).toBeUndefined();
    expect(bts.reload(()=>{},callback)).toBeUndefined();
    // What the transport refuses is a value it has no encoding for. Node's
    // `structuredClone` stand-in throws a DataCloneError where the engine
    // throws a TypeError, so only the refusal itself is asserted.
    const refused: {hook?: unknown}={}; refused.hook=()=>{};
    expect(()=>bts.reload(refused,callback)).toThrow();
    expect(toMain).toHaveLength(start);
    expect(callback).not.toHaveBeenCalled();
  });

  it("delivers a zero-argument BTS reload callback after MTS jobs and first-screen notification", async () => {
    const oldProcess=scope.processData, oldRemove=scope.removeComponents, oldUpdate=scope.updatePage;
    const app=bts.getApp(), oldReload=app.onAppReload, oldLifecycle=app.OnLifecycleEvent;
    const order: unknown[]=[];
    const callback=rstest.fn(function(this: unknown, ...args: unknown[]) {
      expect(this).toBeUndefined(); expect(args).toEqual([]); order.push('callback');
    });
    try {
      scope.processData=()=>{throw Error('BTS reload must not process MTS data');};
      scope.removeComponents=()=>{order.push('remove');};
      scope.updatePage=(data, options)=>{
        expect(options).toEqual({resetPageData:false,reloadFromJS:true,reloadTemplate:true,nativeUpdateDataOrder:0});
        order.push(['render',data]);
        mts.__OnLifecycleEvent(['reload-first',{seed:(data as {seed: number}).seed}]);
        Promise.resolve().then(()=>order.push('main job'));
      };
      app.onAppReload=function(...args){expect(this).toBe(app);order.push(['background',...args]);};
      app.OnLifecycleEvent=(...args)=>{order.push(['lifecycle',...args]);};
      expect(bts.reload({seed:2},callback)).toBeUndefined();
      order.push('returned');
      expect(callback).not.toHaveBeenCalled();
      await deliverToMain();
      expect(order).toEqual(['returned','remove',['render',{seed:2}],'main job']);
      deliverToBackground();
      deliverToBackground();
      const reply=toBackground.shift();
      receiveInBackground({data:reply});
      receiveInBackground({data:reply});
      expect(callback).toHaveBeenCalledTimes(1);
      expect(order.slice(4)).toEqual([
        ['background',{seed:2},{processorName:''}],
        ['lifecycle',['reload-first',{seed:2}]],'callback',
      ]);
      scope.updatePage=()=>{throw Error('reload render failed');};
      const throws=rstest.fn(()=>{throw Error('reload callback failed');});
      bts.reload(null,throws);
      await deliverToMain();
      expect(reportedErrors).toHaveBeenLastCalledWith('error',expect.stringContaining('reload render failed'));
      deliverToBackground();
      const failureReply=toBackground.shift();
      expect(()=>receiveInBackground({data:failureReply})).toThrow('reload callback failed');
      expect(()=>receiveInBackground({data:failureReply})).not.toThrow();
      expect(throws).toHaveBeenCalledTimes(1);
    } finally {
      scope.processData=oldProcess; scope.removeComponents=oldRemove; scope.updatePage=oldUpdate;
      if(oldReload)app.onAppReload=oldReload;else delete app.onAppReload;
      if(oldLifecycle)app.OnLifecycleEvent=oldLifecycle;else delete app.OnLifecycleEvent;
    }
  });

  it("exposes one BTS event module and directly delivers accepted host argument lists", () => {
    const emitter = bts.getJSModule("GlobalEventEmitter") as globalEventEmitter.GlobalEventEmitter;
    expect(emitter).toBe(bts.getApp().GlobalEventEmitter);
    expect(emitter).toBe(bts.getApp().getJSModule("GlobalEventEmitter"));
    expect(bts.getJSModule("missing")).toBeUndefined();
    const registered = {};
    bts.registerModule("custom-module", registered);
    expect(bts.getApp().getJSModule("custom-module")).toBe(registered);
    const listener = rstest.fn();
    emitter.addListener("host-event", listener);
    mts.__BobcatSendGlobalEvent("host-event", JSON.stringify([1, {value: 2}]));
    expect(toBackground).toHaveLength(1);
    deliverToBackground();
    expect(listener).toHaveBeenCalledWith(1, {value: 2});
    mts.__BobcatSendGlobalEvent("host-event", "[]");
    deliverToBackground();
    expect(listener.mock.calls).toEqual([[1, {value: 2}], []]);
    emitter.removeAllListeners("host-event");
  });

  it("reports both realms' diagnostics directly, with severity, values and Error stacks", async () => {
    reportedErrors.mockClear();
    consoleMessages.mockClear();
    const error = new Error("render failed");
    mts._ReportError(error, {level: "warning"});
    expect(reportedErrors).toHaveBeenLastCalledWith("warn", expect.stringContaining("render failed"));
    mts.lynx.reportError("still running", {level: "fatal"});
    expect(reportedErrors).toHaveBeenLastCalledWith("fatal", "still running");
    mts.console["log"]?.("MTS", {value: 1}, undefined);
    expect(consoleMessages).toHaveBeenLastCalledWith("log", 'MTS {"value":1} undefined');
    const {console: backgroundConsole} = await import("../src/background-thread-runtime.ts");
    // Both runtimes' bindings are `bobcat:diagnostics`'s, which this suite
    // loads once for the two of them: what the BTS reports goes to the host
    // itself, never to the main thread as a message to forward.
    expect(backgroundConsole).toBe(mts.console);
    expect(bts.reportError).toBe(mts._ReportError);
    bts.reportError(error, {level: "invalid"});
    bts.reportError(null, {level: "warning"});
    backgroundConsole["warn"]?.("BTS", [1, 2]);
    expect(reportedErrors.mock.calls.slice(2)).toEqual([
      ["error", expect.stringContaining("render failed")], ["warn", "null"],
    ]);
    expect(reportedErrors.mock.calls[2]?.[1]).toContain(error.stack);
    expect(consoleMessages).toHaveBeenLastCalledWith("warn", "BTS [1,2]");
    const circular: {self?: unknown} = {}; circular.self = circular;
    mts.console["debug"]?.(circular);
    expect(consoleMessages).toHaveBeenLastCalledWith("debug", "[object Object]");
    expect(toMain).toHaveLength(0);
  });
});

it("leaves a BTS worker error to the host, which already reports every one", () => {
  const before = reportedErrors.mock.calls.length;
  expect(() => worker.dispatchEvent({type: "error", message: "BTS entry failed"})).not.toThrow();
  expect(reportedErrors.mock.calls).toHaveLength(before);
});

it("waits for the JS disposal acknowledgement before terminating the Worker", async () => {
  const app = bts.getApp();
  const calls: string[] = [];
  app.callDestroyLifetimeFun = function (...args) {
    expect(this).toBe(app);
    expect(args).toEqual([]);
    calls.push("hook");
    Promise.resolve().then(() => calls.push("job"));
    throw Error("BTS destroy");
  };
  const disposing = mts.__BobcatDispose();
  mts.lynx.getEngine().dispatchEvent({type: "__DestroyLifetime"});
  expect(toBackground).toEqual([{bobcat: "runtime", method: "dispose"}]);
  expect(worker.terminate).not.toHaveBeenCalled();
  await deliverToBackground();
  expect(calls).toEqual(["hook", "job"]);
  expect(worker.terminate).not.toHaveBeenCalled();
  // The hook's throw reached the host during the hook; the one message is
  // the disposal acknowledgement.
  expect(reportedErrors).toHaveBeenLastCalledWith("error", expect.stringContaining("BTS destroy"));
  await deliverToMain();
  await disposing;
  expect(worker.terminate).toHaveBeenCalledTimes(1);
  expect(toBackground).toHaveLength(0);
});

it("reports a BTS entry that throws and keeps taking messages after it", async () => {
  const runtime = await import("../src/background-thread-runtime.ts");
  const reportError = rstest.fn();
  scope.reportError = reportError;
  const failure = Error("BTS entry failed");
  runtime.__BobcatStartBTS(() => Promise.reject(failure));
  const received: unknown[] = [];
  const emitter = bts.getJSModule("GlobalEventEmitter") as globalEventEmitter.GlobalEventEmitter;
  emitter.addListener("after-failure", (value: unknown) => { received.push(value); });
  // The entry's rejection must not stop the message behind it: both are
  // delivered before either settles, as the Worker queue delivers them.
  const initializing = receiveInBackground({data: {
    bobcat: "runtime", method: "initialize", updateData: {}, systemInfo: {},
  }});
  const delivering = receiveInBackground({data: {
    bobcat: "runtime", method: "sendGlobalEvent", name: "after-failure", args: [1],
  }});
  await initializing;
  await delivering;
  expect(reportError).toHaveBeenCalledExactlyOnceWith(failure);
  expect(received).toEqual([1]);
  delete scope.reportError;
});

// Last, and on their own MTS instance: the runtime is a module singleton, and
// the disposal test above has already taken this file's instance past
// disposal, after which it sends nothing at all.
describe("a BTS Worker that ended", () => {
  let runtime: typeof mtsRuntime;
  const endedWorker = Object.assign(new eventTarget.EventTarget(), {
    terminate: rstest.fn(),
    postMessage: rstest.fn(),
  });

  beforeAll(async () => {
    // A second boot of the same source. The `bobcat:*` modules it imports are
    // this file's own objects, so its EventTarget, its Context class and its
    // host bindings are the ones every other test here uses.
    rstest.resetModules();
    runtime = await import("../src/main-thread-runtime.ts");
    runtime.__BobcatConnectBackground(endedWorker as unknown as Worker, {});
    endedWorker.postMessage.mockClear();
    endedWorker.dispatchEvent({type: "__bobcat:close"});
  });

  it("keeps posting to the ended Worker, where the host drops the message", () => {
    runtime.__OnLifecycleEvent(["lifecycle", 1]);
    runtime.__BobcatSendGlobalEvent("x", "[]");
    // Nothing was swallowed by an internal queue: both crossed to the Worker,
    // as a browser lets a post to a terminated worker cross and be dropped.
    expect(endedWorker.postMessage.mock.calls).toEqual([
      [{type: "__OnLifecycleEvent", data: ["lifecycle", 1], origin: "CoreContext"}],
      [{bobcat: "runtime", method: "sendGlobalEvent", name: "x", args: []}],
    ]);
  });

  it("disposes without a `dispose` no one could answer", async () => {
    await runtime.__BobcatDispose();
    expect(endedWorker.postMessage).toHaveBeenCalledTimes(2);
    expect(endedWorker.terminate).toHaveBeenCalledTimes(1);
  });
});
