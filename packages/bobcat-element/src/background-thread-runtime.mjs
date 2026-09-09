// @ts-check
import "bobcat:worker";
import { createCrossThreadContext } from "bobcat:cross-thread-context";

// The bobcat:bts bootstrap and the BTS application's entry preamble import
// this runtime. Like MTS, lynx is a module binding, never a global property.
// Application module loading through ResourceFetcher remains pending.
/** @type {any} */
const scope = globalThis;
const coreContext = createCrossThreadContext();

/** @typedef {(...args: any[]) => unknown} AppHook */
/** @type {{
 * OnLifecycleEvent?: AppHook,
 * publishEvent?: AppHook,
 * publicComponentEvent?: AppHook,
 * callDestroyLifetimeFun?: AppHook,
 * }} */
const app = {};
/** @type {Map<number, (result: unknown) => void>} */
const callbacks = new Map();
let nextCallbackId = 1;

/**
 * web-core registers these two handlers lazily: an event received before the
 * framework installs its hook waits for that hook. Later calls read the current
 * property because ReactLynx replaces the initial handler during setup.
 * @param {"publishEvent" | "publicComponentEvent"} name
 */
function createPublishHandler(name) {
  /** @type {AppHook | undefined} */
  let handler;
  /** @type {any[][]} */
  let pending = [];
  /** @param {any[]} args */
  async function replay(args) {
    // As in web-core's async RPC receiver, call the current hook immediately,
    // but report a rejection independently so later queued events still run.
    return app[name]?.apply(app, args);
  }
  Object.defineProperty(app, name, {
    get() { return handler; },
    /** @param {AppHook | undefined} value */
    set(value) {
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
  /** @param {any[]} args */
  return (args) => {
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

const nativeApp = {
  /**
   * @param {string} name
   * @param {unknown} data
   * @param {(result: unknown) => void} [callback]
   */
  callLepusMethod(name, data, callback) {
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

coreContext.addEventListener(
  "__OnLifecycleEvent",
  (/** @type {{data: unknown}} */ event) => {
    app.OnLifecycleEvent?.call(app, event.data);
  },
);

coreContext.connect((event) => scope.postMessage({ type: event.type, data: event.data }));
scope.addEventListener("message", (/** @type {{data: any}} */ event) => {
  const message = event.data;
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
    case "callDestroyLifetimeFun":
      app.callDestroyLifetimeFun?.call(app);
      break;
    case "callLepusMethodResult": {
      const callback = callbacks.get(message.id);
      callbacks.delete(message.id);
      if (message.error !== undefined) {
        const error = new Error(message.error.message);
        error.name = message.error.name;
        throw error;
      }
      callback?.(message.result);
      break;
    }
  }
});

// This is the raw BTS environment's MVP. Loading a compiled ReactLynx BTS
// bundle also needs Lynx Core's module/init shell, which is not installed here.
export const lynx = {
  getApp() {
    return app;
  },
  getNativeApp() {
    return nativeApp;
  },
  getCoreContext() {
    return coreContext;
  },
};
