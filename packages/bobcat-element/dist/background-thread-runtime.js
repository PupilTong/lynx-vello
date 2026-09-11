// Generated from src/background-thread-runtime.ts by TypeScript 7: edit that file and run `pnpm --filter bobcat-element build`.
// source fnv1a64 b73ba82f3b0ed615
import "bobcat:worker";
import { createCrossThreadContext, packBtsMessage, unpackBtsMessage, cloneBtsValue, } from "bobcat:cross-thread-context";
// The bobcat:bts bootstrap and the BTS application's entry preamble import
// this runtime. Like MTS, lynx is a module binding, never a global property.
// Application module loading through ResourceFetcher remains pending.
const scope = globalThis;
function postToMain(message) { scope.postMessage(packBtsMessage(message)); }
const coreContext = createCrossThreadContext();
const app = {};
// Looked up by the id a `callLepusMethodResult` carries, which a result for
// a call made without a callback lacks.
const callbacks = new Map();
let nextCallbackId = 1;
/**
 * web-core registers these two handlers lazily: an event received before the
 * framework installs its hook waits for that hook. Later calls read the current
 * property because ReactLynx replaces the initial handler during setup.
 */
function createPublishHandler(name) {
    let handler;
    let pending = [];
    async function replay(args) {
        // As in web-core's async RPC receiver, call the current hook immediately,
        // but report a rejection independently so later queued events still run.
        return app[name]?.apply(app, args);
    }
    Object.defineProperty(app, name, {
        get() { return handler; },
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
    return (args) => {
        const current = app[name];
        if (typeof current === "function") {
            current.apply(app, args);
        }
        else {
            pending.push(args);
        }
    };
}
const publishEvent = createPublishHandler("publishEvent");
const publicComponentEvent = createPublishHandler("publicComponentEvent");
const nativeApp = {
    callLepusMethod(name, data, callback) {
        if (arguments.length < 2)
            throw new TypeError("callLepusMethod requires name and data");
        if (typeof name !== "string")
            name = "";
        const snapshot = cloneBtsValue(data);
        if (snapshot === null || typeof snapshot !== "object")
            return;
        let id;
        if (typeof callback === "function") {
            id = nextCallbackId++;
            callbacks.set(id, callback);
        }
        try {
            postToMain({
                bobcat: "runtime", method: "callLepusMethod", name, data: snapshot, id,
            });
        }
        catch (error) {
            if (id !== undefined)
                callbacks.delete(id);
            throw error;
        }
    },
};
// web-worker-rpc callbackify invokes callbacks in a Promise continuation.
// Release the ID before scheduling it, so duplicate replies cannot invoke it twice.
async function receiveLepusResult(message) {
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
coreContext.addEventListener("__OnLifecycleEvent", (event) => {
    app.OnLifecycleEvent?.call(app, event.data);
});
coreContext.connect((event) => postToMain({ type: event.type, data: event.data }));
scope.addEventListener("message", (event) => {
    const message = unpackBtsMessage(event.data);
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
        case "callLepusMethodResult":
            return receiveLepusResult(message);
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
