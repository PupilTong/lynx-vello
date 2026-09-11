import { afterAll, beforeAll, describe, expect, it, rstest } from "@rstest/core";
import * as eventTarget from "../src/event-target.ts";
import * as crossThreadContext from "../src/cross-thread-context.ts";
import type * as btsRuntime from "../src/background-thread-runtime.ts";
import type * as mtsRuntime from "../src/main-thread-runtime.ts";
import type { Worker } from "../src/worker.ts";

rstest.mockRequire("bobcat:event-target", () => eventTarget);
rstest.mockRequire("bobcat:cross-thread-context", () => crossThreadContext);
rstest.mockRequire("bobcat:worker", () => ({}));
// The runtime reads the view's page data as it evaluates; this view has none.
rstest.mockRequire("bobcat-internal:host", () => ({
  initData: () => undefined,
  globalProps: () => undefined,
}));

/**
 * The members of the realm global the two runtimes reach, as this suite
 * installs them on Node's `globalThis`, plus the Lepus methods a test files
 * there for `callLepusMethod` to find.
 */
interface TestScope {
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
  bigIntLepusMethod?: (data: unknown) => unknown;
}

/** A message for the background realm, as the transport's JSON copy of it. */
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
let receiveInBackground: (event: { data: unknown }) => void | Promise<void>;
const toBackground: Recorded[] = [];
const toMain: unknown[] = [];
const worker = Object.assign(new eventTarget.EventTarget(), {
  postMessage(message: unknown) {
    toBackground.push(crossThreadContext.unpackBtsMessage(JSON.parse(JSON.stringify([message]))[0]) as Recorded);
  },
});

beforeAll(async () => {
  mts = await import("../src/main-thread-runtime.ts");
  scope.emptyLepusMethod = () => undefined;
  scope.postMessage = (message: unknown) => {
    toMain.push(crossThreadContext.unpackBtsMessage(JSON.parse(JSON.stringify([message]))[0]));
  };
  scope.addEventListener = (name: string, callback) => {
    if (name === "message") receiveInBackground = callback;
  };
  ({ lynx: bts } = await import("../src/background-thread-runtime.ts"));
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
    mts.__BobcatConnectBackground(worker as unknown as Worker);
    expect(toBackground.map((message) => message.method ?? message.type)).toEqual([
      "publishEvent", "custom", "publicComponentEvent",
    ]);
    expect(toBackground[1]).toEqual({ type: "custom", data: 0 });
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
    expect(toBackground[0]).toEqual({ type: "protocol-like", data: 7 });
    deliverToBackground();
    bts.getCoreContext().dispatchEvent(event);
    expect(toMain[0]).toEqual({ type: "protocol-like", data: 7 });
    await deliverToMain();
    expect(seen).toEqual([
      ["BTS", { type: "protocol-like", data: 7 }],
      ["MTS", { type: "protocol-like", data: 7 }],
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

  it("ignores primitive RPC arguments but preserves null and undefined inside objects", async () => {
    const callback = rstest.fn();
    scope.inspectLepusData = (data: unknown) => data;
    try {
      for (const data of [undefined, null, false, 42, "text"]) {
        bts.getNativeApp().callLepusMethod("inspectLepusData", data, callback);
      }
      expect(toMain).toEqual([]);
      const data: { missing: undefined; nil: null | string } = {missing: undefined, nil: null};
      bts.getNativeApp().callLepusMethod("inspectLepusData", data, callback);
      data.nil = "changed after send";
      await deliverToMain(); await deliverToBackground();
      expect(callback.mock.calls).toEqual([[{missing: undefined, nil: null}]]);
      expect(Object.hasOwn(callback.mock.calls[0]?.[0], "missing")).toBe(true);
    } finally { delete scope.inspectLepusData; }
  });

  it("releases callbacks on failed sends and reports result encoding failures", async () => {
    const callback = rstest.fn();
    expect(() => bts.getNativeApp().callLepusMethod("emptyLepusMethod", 1n, callback)).toThrow();
    const postMessage = scope.postMessage;
    let failedId: number | undefined;
    scope.postMessage = (message: unknown) => {
      failedId = (crossThreadContext.unpackBtsMessage(message) as { id?: number }).id;
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
    scope.bigIntLepusMethod = () => 1n;
    try {
      bts.getNativeApp().callLepusMethod("bigIntLepusMethod", {}, callback);
      await deliverToMain();
      await expect(deliverToBackground()).rejects.toThrow();
      expect(callback).not.toHaveBeenCalled();
    } finally { delete scope.bigIntLepusMethod; }
  });


  it("forwards explicit destroy events to the current app hook without disposing the runtime", async () => {
    const callback = rstest.fn();
    bts.getNativeApp().callLepusMethod("emptyLepusMethod", {}, callback);
    await deliverToMain();
    const lateReply = toBackground.shift();
    const engine = mts.lynx.getEngine();
    const app = bts.getApp();
    app.callDestroyLifetimeFun = function (...args) {
      expect(this).toBe(app);
      expect(args).toEqual([]);
      throw new Error("BTS destroy");
    };
    engine.dispatchEvent({ type: "__DestroyLifetime", data: "ignored" });
    expect(toBackground).toEqual([{ bobcat: "runtime", method: "callDestroyLifetimeFun" }]);
    expect(() => deliverToBackground()).toThrow("BTS destroy");
    await receiveInBackground({ data: lateReply });
    expect(callback).toHaveBeenCalledTimes(1);
    expect(callback).toHaveBeenCalledWith(undefined);

    const replacement = rstest.fn(function (this: typeof app, ...args: unknown[]) {
      expect(this).toBe(app);
      expect(args).toEqual([]);
    });
    app.callDestroyLifetimeFun = replacement;
    engine.dispatchEvent({ type: "__DestroyLifetime" });
    deliverToBackground();
    expect(replacement).toHaveBeenCalledTimes(1);

    bts.getNativeApp().callLepusMethod("emptyLepusMethod", {}, callback);
    await deliverToMain();
    await deliverToBackground();
    expect(callback).toHaveBeenCalledTimes(2);
  });
});
