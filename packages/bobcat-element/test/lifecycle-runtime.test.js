// @ts-check

import { afterAll, beforeAll, describe, expect, it, rstest } from "@rstest/core";
import * as eventTarget from "../src/event-target.mjs";
import * as crossThreadContext from "../src/cross-thread-context.mjs";

rstest.mockRequire("bobcat:event-target", () => eventTarget);
rstest.mockRequire("bobcat:cross-thread-context", () => crossThreadContext);
rstest.mockRequire("bobcat:worker", () => ({}));
// The runtime reads the view's page data as it evaluates; this view has none.
rstest.mockRequire("bobcat-internal:host", () => ({
  initData: () => undefined,
  globalProps: () => undefined,
}));

/** @type {any} */
const scope = globalThis;
const originalPostMessage = scope.postMessage;
const originalAddEventListener = scope.addEventListener;
/** @type {typeof import("../src/main-thread-runtime.mjs")} */
let mts;
/** @type {typeof import("../src/background-thread-runtime.mjs").lynx} */
let bts;
/** @type {(event: {data: any}) => void} */
let receiveInBackground;
/** @type {any[]} */
const toBackground = [];
/** @type {any[]} */
const toMain = [];
const worker = Object.assign(new eventTarget.EventTarget(), {
  /** @param {any} message */
  postMessage(message) {
    toBackground.push(JSON.parse(JSON.stringify([message]))[0]);
  },
});

beforeAll(async () => {
  mts = await import("../src/main-thread-runtime.mjs");
  scope.postMessage = (/** @type {any} */ message) => {
    toMain.push(JSON.parse(JSON.stringify([message]))[0]);
  };
  scope.addEventListener = (/** @type {string} */ name, /** @type {any} */ callback) => {
    if (name === "message") receiveInBackground = callback;
  };
  ({ lynx: bts } = await import("../src/background-thread-runtime.mjs"));
});

afterAll(() => {
  scope.postMessage = originalPostMessage;
  scope.addEventListener = originalAddEventListener;
});

function deliverToBackground() {
  receiveInBackground({ data: toBackground.shift() });
}

async function deliverToMain() {
  worker.dispatchEvent({ type: "message", data: toMain.shift() });
  // callLepusMethod awaits both synchronous and asynchronous return values.
  await Promise.resolve();
  await Promise.resolve();
}

describe("MTS/BTS lifecycle runtime", () => {
  it("queues Context and publish calls together, then replays each late publish hook", () => {
    /** @type {unknown[]} */
    const seen = [];
    bts.getCoreContext().addEventListener("custom", (/** @type {any} */ event) => {
      seen.push(["context", event.data]);
    });
    const contextEvent = {
      type: "custom", data: 0, bobcat: "runtime", method: "callDestroyLifetimeFun",
    };
    mts.__BobcatPublishEvent(undefined, "first", { value: 1 });
    mts.lynx.getJSContext().dispatchEvent(contextEvent);
    mts.__BobcatPublishEvent("component", "second", { value: 3 });
    contextEvent.data = 2;
    mts.__BobcatConnectBackground(
      /** @type {import("../src/worker.mjs").Worker} */ (/** @type {unknown} */ (worker)),
    );
    expect(toBackground.map((message) => message.method ?? message.type)).toEqual([
      "publishEvent", "custom", "publicComponentEvent",
    ]);
    expect(toBackground[1]).toEqual({ type: "custom", data: 2 });
    deliverToBackground();
    deliverToBackground();
    deliverToBackground();
    expect(seen).toEqual([["context", 2]]);
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
      ["context", 2], ["component", "second", { value: 3 }], ["first", { value: 1 }],
    ]);
    app.publishEvent = function (...args) { seen.push(["replacement", ...args]); };
    mts.__BobcatPublishEvent(0, "third", { value: 4 });
    deliverToBackground();
    expect(seen.at(-1)).toEqual(["replacement", "third", { value: 4 }]);
    expect(bts.getApp()).toBe(app);
    expect(bts.getNativeApp()).toBe(bts.getNativeApp());
  });

  it("keeps extra Context properties separate from runtime messages in both directions", async () => {
    /** @type {unknown[]} */
    const seen = [];
    mts.lynx.getJSContext().addEventListener("protocol-like", (/** @type {any} */ event) => {
      seen.push(["MTS", event]);
    });
    bts.getCoreContext().addEventListener("protocol-like", (/** @type {any} */ event) => {
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

  it("reports rejected methods through BTS and removes failed callback registrations", async () => {
    const callback = rstest.fn();
    scope.failedLepusMethod = async () => { throw new TypeError("method failed"); };
    try {
      bts.getNativeApp().callLepusMethod("failedLepusMethod", {}, callback);
      await deliverToMain();
      const reply = toBackground[0];
      expect(() => deliverToBackground()).toThrow("method failed");
      expect(callback).not.toHaveBeenCalled();
      receiveInBackground({ data: { ...reply, error: undefined, result: "late" } });
      expect(callback).not.toHaveBeenCalled();
      bts.getNativeApp().callLepusMethod("failedLepusMethod", {});
      await deliverToMain();
      expect(() => deliverToBackground()).toThrow("method failed");
    } finally {
      delete scope.failedLepusMethod;
    }
  });

  it("removes callback registrations before invoking a callback that throws", async () => {
    const callback = rstest.fn(() => { throw new Error("callback failed"); });
    bts.getNativeApp().callLepusMethod("missingLepusMethod", {}, callback);
    await deliverToMain();
    const reply = toBackground[0];
    expect(() => deliverToBackground()).toThrow("callback failed");
    expect(callback).toHaveBeenCalledTimes(1);
    receiveInBackground({ data: reply });
    expect(callback).toHaveBeenCalledTimes(1);
  });

  it("keeps missing method data distinct from explicit null through JSON transport", async () => {
    /** @type {unknown[]} */
    const seen = [];
    scope.inspectLepusData = (/** @type {unknown} */ data) => {
      seen.push(data);
      return data;
    };
    const callback = rstest.fn();
    try {
      bts.getNativeApp().callLepusMethod("inspectLepusData", undefined, callback);
      await deliverToMain();
      deliverToBackground();
      bts.getNativeApp().callLepusMethod("inspectLepusData", null, callback);
      await deliverToMain();
      deliverToBackground();
      expect(seen).toEqual([undefined, null]);
      expect(callback.mock.calls).toEqual([[undefined], [null]]);
    } finally {
      delete scope.inspectLepusData;
    }
  });

  it("cleans callbacks when request encoding fails and reports result encoding failures", async () => {
    const callback = rstest.fn();
    const postMessage = scope.postMessage;
    /** @type {number | undefined} */
    let failedId;
    scope.postMessage = (/** @type {any} */ message) => {
      failedId = message.id;
      postMessage(message);
    };
    try {
      expect(() => bts.getNativeApp().callLepusMethod("missingLepusMethod", 1n, callback)).toThrow();
      receiveInBackground({ data: {
        bobcat: "runtime", method: "callLepusMethodResult", id: failedId, result: 1,
      } });
      expect(callback).not.toHaveBeenCalled();
    } finally {
      scope.postMessage = postMessage;
    }
    scope.bigIntLepusMethod = () => 1n;
    try {
      bts.getNativeApp().callLepusMethod("bigIntLepusMethod", {}, callback);
      await deliverToMain();
      expect(() => deliverToBackground()).toThrow();
      expect(callback).not.toHaveBeenCalled();
    } finally {
      delete scope.bigIntLepusMethod;
    }
  });

  it("forwards explicit destroy events to the current app hook without disposing the runtime", async () => {
    const callback = rstest.fn();
    bts.getNativeApp().callLepusMethod("missingLepusMethod", {}, callback);
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
    receiveInBackground({ data: lateReply });
    expect(callback).toHaveBeenCalledTimes(1);
    expect(callback).toHaveBeenCalledWith(undefined);

    const replacement = rstest.fn(/** @this {typeof app} */ function (...args) {
      expect(this).toBe(app);
      expect(args).toEqual([]);
    });
    app.callDestroyLifetimeFun = replacement;
    engine.dispatchEvent({ type: "__DestroyLifetime" });
    deliverToBackground();
    expect(replacement).toHaveBeenCalledTimes(1);

    bts.getNativeApp().callLepusMethod("missingLepusMethod", {}, callback);
    await deliverToMain();
    deliverToBackground();
    expect(callback).toHaveBeenCalledTimes(2);
  });
});
