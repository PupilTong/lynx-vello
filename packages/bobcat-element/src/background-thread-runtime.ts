import "bobcat:worker";
import type { WorkerGlobalScope } from "bobcat:worker";
import {
  type ContextEvent,
  createCrossThreadContext,
} from "bobcat:cross-thread-context";

// The bobcat:bts bootstrap and the BTS application's entry preamble import
// this runtime. Like MTS, lynx is a module binding, never a global property.
// Application module loading through ResourceFetcher remains pending.
const scope = globalThis as unknown as WorkerGlobalScope;
const coreContext = createCrossThreadContext();

type AppHook = (...args: unknown[]) => unknown;
const app: {
  OnLifecycleEvent?: AppHook;
  publishEvent?: AppHook;
  publicComponentEvent?: AppHook;
  callDestroyLifetimeFun?: AppHook;
} = {};
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
  | {
      bobcat: "runtime";
      method: "publishEvent" | "publicComponentEvent";
      args: unknown[];
    }
  | { bobcat: "runtime"; method: "callDestroyLifetimeFun" }
  | {
      bobcat: "runtime";
      method: "callLepusMethodResult";
      id?: number;
      result?: unknown;
      error?: { name: string; message: string };
    }
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

const nativeApp = {
  callLepusMethod(
    name: string,
    data: unknown,
    callback?: (result: unknown) => void,
  ) {
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
  (event: { data: unknown }) => {
    app.OnLifecycleEvent?.call(app, event.data);
  },
);

coreContext.connect((event) => scope.postMessage({ type: event.type, data: event.data }));
scope.addEventListener("message", (event: { data: FromMainThread }) => {
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
