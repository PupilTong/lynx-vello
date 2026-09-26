import { afterAll, beforeAll, describe, expect, it, rstest } from "@rstest/core";
import * as eventTarget from "../src/event-target.ts";
import * as crossThreadContext from "../src/cross-thread-context.ts";
import type * as btsRuntime from "../src/background-thread-runtime.ts";
import type * as workerRuntime from "../src/worker-runtime.ts";
import type * as nativeModulesRuntime from "../src/native-modules.ts";
import * as selectorQuery from "../src/selector-query.ts";
import * as lynxModules from "../src/lynx-modules.ts";
import * as globalEventEmitter from "../src/global-event-emitter.ts";

// The transport, `bobcat:native-modules`, is the module under test here, so
// it is loaded rather than mocked; only the host functions below it are stood
// in for.
const invokeNativeModule = rstest.fn();
rstest.mockRequire("bobcat-internal:worker", () => ({
  postWorkerMessage: rstest.fn(),
  closeWorker: rstest.fn(),
  workerName: () => "",
}));
rstest.mockRequire("bobcat-internal:native-modules", () => ({ invokeNativeModule }));
import * as record from "../src/record.ts";
rstest.mockRequire("bobcat:record", () => record);
rstest.mockRequire("bobcat:lynx-modules", () => lynxModules);
import * as sectionUrl from "../src/section-url.ts";
import * as future from "../src/future.ts";
import * as bundleFetch from "../src/bundle-fetch.ts";
import * as diagnostics from "../src/diagnostics.ts";
rstest.mockRequire("bobcat:global-event-emitter", () => globalEventEmitter);
rstest.mockRequire("bobcat:selector-query", () => selectorQuery);
rstest.mockRequire("bobcat:event-target", () => eventTarget);
rstest.mockRequire("bobcat:cross-thread-context", () => crossThreadContext);
rstest.mockRequire("bobcat:timers", () => ({}));
rstest.mockRequire("bobcat:element", () => ({ __BobcatQueryNodes: rstest.fn() }));
import * as animationFrame from "../src/animation-frame.ts";
import * as systemInfo from "../src/system-info.ts";
rstest.mockRequire("bobcat:animation-frame", () => animationFrame);
rstest.mockRequire("bobcat:system-info", () => systemInfo);
// The transport and the worker realm's global scope, as the BTS runtime
// imports them — the real modules. Answered lazily: the mock registrations are
// hoisted above this file's own bindings, and the modules are imported from
// `beforeAll` once those exist.
rstest.mockRequire("bobcat:native-modules", () => transport);
rstest.mockRequire("bobcat:worker", () => worker);
rstest.mockRequire("bobcat:section-url", () => sectionUrl);
rstest.mockRequire("bobcat:future", () => future);
rstest.mockRequire("bobcat:bundle-fetch", () => bundleFetch);
rstest.mockRequire("bobcat:diagnostics", () => diagnostics);
rstest.mockRequire("bobcat-internal:host", () => ({
  requestScriptFrame: rstest.fn(),
  reportScriptError: rstest.fn(),
  logScriptMessage: rstest.fn(),
  preloadStyleSheet: rstest.fn(),
  adoptStyleSheet: rstest.fn(),
  initialProcessor: () => "",
  initData: () => undefined,
  globalProps: () => undefined,
  // The modules table imports both; every suite below serves registered
  // sources, so nothing here reaches an external load.
  resolveModuleUrl: () => { throw new Error("no module resolution in this suite"); },
  loadModuleSync: () => { throw new Error("no module load in this suite"); },
  // No suite here fetches anything, and so registers no future; the runtime
  // only needs these to exist, because `lynx.fetchBundle` and the `Future`
  // class close over them as it evaluates.
  fetchResource: () => { throw new Error("no fetch in this suite"); },
  waitFuture: () => { throw new Error("no future in this suite"); },
  takeFuture: () => { throw new Error("no future in this suite"); },
  settleFuture: () => { throw new Error("no future in this suite"); },
}));

/** What the host hands `invokeNativeModule`, in argument order. */
type Invocation = [
  call: number,
  module: string,
  method: string,
  args: string,
  callbacks: string,
];

let transport: typeof nativeModulesRuntime;
let worker: typeof workerRuntime;
let bts: typeof btsRuntime;
// The real worker scope defines the realm's own `console` on the global it
// runs against, which here is Node's: this is Node's, put back afterwards.
const nodeConsole = Object.getOwnPropertyDescriptor(globalThis, "console")!;

// Nothing stands in for the global's `postMessage` and `addEventListener`:
// the real `bobcat:worker` defines both on Node's global, and the BTS runtime
// hears `initialize` through the second.
beforeAll(async () => {
  transport = await import("../src/native-modules.ts");
  worker = await import("../src/worker-runtime.ts");
  bts = await import("../src/background-thread-runtime.ts");
});

afterAll(() => {
  Object.defineProperty(globalThis, "console", nodeConsole);
  // The BTS runtime's own global, which it defines on Node's.
  delete (globalThis as { SystemInfo?: unknown }).SystemInfo;
});

/** The most recent call the host was handed. */
function lastInvocation(): Invocation {
  const calls = invokeNativeModule.mock.calls;
  return calls[calls.length - 1] as Invocation;
}

describe("NativeModules transport", () => {
  it("sends the arguments as JSON with each function argument as null", () => {
    const callback = rstest.fn();
    expect(transport.callNativeModule("Echo", "ping", [1, { x: 1 }, callback, "s"]))
      .toBeUndefined();
    const [call, module, method, args, callbacks] = lastInvocation();
    expect([module, method]).toEqual(["Echo", "ping"]);
    expect(args).toBe('[1,{"x":1},null,"s"]');
    expect(callbacks).toBe("2");
    expect(typeof call).toBe("number");
  });

  it("names every function argument and nothing else", () => {
    transport.callNativeModule("Echo", "pair", [() => undefined, 0, () => undefined]);
    expect(lastInvocation()[4]).toBe("0,2");
    transport.callNativeModule("Echo", "none", ["only text"]);
    expect(lastInvocation()[4]).toBe("");
  });

  it("invokes a callback once, with the spread JSON answer", () => {
    const callback = rstest.fn();
    transport.callNativeModule("Echo", "ping", [callback]);
    const call = lastInvocation()[0];
    transport.__BobcatNativeModuleCallback(call, 0, '["pong",2]');
    expect(callback.mock.calls).toEqual([["pong", 2]]);
    // Single-shot, as native's CallbackImpl is: the slot was cleared before
    // the function ran, so a second answer finds nothing.
    transport.__BobcatNativeModuleCallback(call, 0, '["again"]');
    expect(callback.mock.calls).toEqual([["pong", 2]]);
  });

  it("keeps each function argument of one call apart", () => {
    const success = rstest.fn();
    const failure = rstest.fn();
    transport.callNativeModule("Echo", "pair", [success, failure]);
    const call = lastInvocation()[0];
    transport.__BobcatNativeModuleCallback(call, 1, '["no"]');
    expect(failure.mock.calls).toEqual([["no"]]);
    expect(success).not.toHaveBeenCalled();
    transport.__BobcatNativeModuleCallback(call, 0, "[]");
    expect(success.mock.calls).toEqual([[]]);
  });

  it("releases a function without calling it when the module answers nothing", () => {
    const callback = rstest.fn();
    transport.callNativeModule("Echo", "drop", [callback]);
    const call = lastInvocation()[0];
    transport.__BobcatNativeModuleCallback(call, 0, undefined);
    expect(callback).not.toHaveBeenCalled();
    transport.__BobcatNativeModuleCallback(call, 0, '["late"]');
    expect(callback).not.toHaveBeenCalled();
  });

  it("registers nothing for a call whose arguments will not serialize", () => {
    invokeNativeModule.mockClear();
    const stranded = rstest.fn();
    expect(() => transport.callNativeModule("Echo", "ping", [1n, stranded]))
      .toThrow(TypeError);
    expect(invokeNativeModule).not.toHaveBeenCalled();
    // The next call gets the id the failed one never took, and answering it
    // reaches its own function rather than the stranded one.
    const callback = rstest.fn();
    transport.callNativeModule("Echo", "ping", [callback]);
    expect(invokeNativeModule).toHaveBeenCalledTimes(1);
    const call = lastInvocation()[0];
    expect(typeof call).toBe("number");
    transport.__BobcatNativeModuleCallback(call, 0, "[]");
    expect(callback).toHaveBeenCalledTimes(1);
    expect(stranded).not.toHaveBeenCalled();
  });

  it("reports a throwing callback the way an uncaught exception is reported", () => {
    // Through the reporter the realm's runtime installed, which the transport
    // reads at the call: `bobcat:worker` installed the value its global's
    // `reportError` had as it was evaluated, so replacing that property now
    // would not reach it.
    const reportError = rstest.fn();
    eventTarget.installExceptionReporter(reportError);
    const failure = Error("module callback failed");
    transport.callNativeModule("Echo", "ping", [() => { throw failure; }]);
    transport.__BobcatNativeModuleCallback(lastInvocation()[0], 0, "[]");
    expect(reportError).toHaveBeenCalledWith(failure);
  });
});

describe("the BTS NativeModules object", () => {
  it("is built out of the table `initialize` carries, before the entry runs", () => {
    const modules = bts.lynx.getApp().NativeModules;
    // Empty as the runtime evaluates, which is all a plain `Worker` that
    // imports it ever sees: nothing posts one an `initialize`.
    expect(Object.keys(modules)).toEqual([]);
    const seen: unknown[] = [];
    bts.__BobcatStartBTS(async () => {
      seen.push(Object.keys(bts.NativeModules), bts.SystemInfo["pixelRatio"]);
    });
    // The table the MTS realm posts for a view built with one module, `Foo`,
    // declaring one method, `bar`, beside that realm's own `SystemInfo`.
    worker.__BobcatDeliverWorkerMessage({
      bobcat: "runtime", method: "initialize", updateData: {},
      systemInfo: systemInfo.createSystemInfo({ pixelRatio: 1.1, pixelWidth: 1287, pixelHeight: 2785 }),
      nativeModuleTable: "3:Foo3:bar",
    });
    // The entry ran with both in place, and the object it saw is the one
    // every name for `NativeModules` already held.
    expect(seen).toEqual([["Foo"], 1.1]);
    expect(bts.NativeModules).toBe(modules);
    expect(bts.lynx.SystemInfo).toBe(bts.SystemInfo);
    expect((globalThis as { SystemInfo?: unknown }).SystemInfo).toBe(bts.SystemInfo);
  });

  it("carries exactly the methods the embedder declared", () => {
    const modules = bts.lynx.getApp().NativeModules as Record<string, Record<string, Function>>;
    expect(Object.keys(modules)).toEqual(["Foo"]);
    invokeNativeModule.mockClear();
    expect(modules["Foo"]!["bar"]!("x")).toBeUndefined();
    expect(lastInvocation().slice(1)).toEqual(["Foo", "bar", '["x"]', ""]);
    // A method the module did not declare is `undefined`, as native answers.
    expect(modules["Foo"]!["nope"]).toBeUndefined();
  });

  it("answers `undefined` for a module the host does not have", () => {
    const modules = bts.lynx.getApp().NativeModules as Record<string, unknown>;
    // web-core's answer — a missing key on a plain object — where native
    // answers `null` from its binding proxy.
    expect(modules["Missing"]).toBeUndefined();
  });

  it("is the same object the native app exposes as `nativeModuleProxy`", () => {
    // The name ReactLynx reads, kept although nothing is a Proxy any more.
    expect(bts.lynx.getNativeApp().nativeModuleProxy)
      .toBe(bts.lynx.getApp().NativeModules);
  });
});
