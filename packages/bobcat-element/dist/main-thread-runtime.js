// Generated from src/main-thread-runtime.ts by TypeScript 7: edit that file and run `pnpm --filter bobcat-element build`.
// source fnv1a64 54dfc0c9164b17a8
// The `bobcat:runtime` compatibility ESM imported by each transformed MTS entry.
//
// The JS Context and lifecycle/event calls reach this view's BTS Worker.
// Native modules and the error reporter remain sinks. A compiled chunk
// still probes those APIs while it installs
// the ReactLynx snapshot runtime, so this module exports explicit sinks for
// that bootstrap surface. The one local delivery path is `lynx.getEngine()`:
// its stable EventTarget retains realm-local listeners so `bobcat:boot` can
// dispatch `__RenderPage` when an entry has no legacy `globalThis.renderPage`.
// None of these bindings is installed on `globalThis`; the entry receives them
// only through the import declarations Bobcat prepends to its source.
//
// The host's page data arrives through `bobcat-internal:host` as the strings
// the view was given, and is parsed here as this module evaluates.
//
// This is not an Element PAPI implementation. Every `__*` element member,
// including the scoped-style sink `__SetCSSId`, belongs to element-papi.ts.
// Background-thread-only bindings such as `lynxCoreInject` also do not belong
// in this realm.
//
// `EventTarget` is imported rather than defined here: the worker realm needs
// the same class, on a runtime this module is not registered on, so the source
// is shared as a module and each runtime compiles its own copy.
import { EventTarget } from "bobcat:event-target";
import { createCrossThreadContext, } from "bobcat:cross-thread-context";
import { globalProps, initData } from "bobcat-internal:host";
function noop() {
    return undefined;
}
function createContextSink() {
    return {
        postMessage: noop,
        addEventListener: noop,
        removeEventListener: noop,
        dispatchEvent: function () {
            // Lynx's ContextProxy result for an event suppressed before delivery.
            return 3;
        },
    };
}
const coreContext = createContextSink();
const jsContext = createCrossThreadContext();
const nativeContext = createContextSink();
const engineContext = new EventTarget();
// The realm's global object, where a card installs the methods
// `callLepusMethod` looks up by name.
const scope = globalThis;
let backgroundWorker;
let pendingBackgroundMessages = [];
function sendToBackground(message, isContext = false) {
    if (backgroundWorker === undefined) {
        pendingBackgroundMessages.push({ message, isContext });
    }
    else {
        // A Context event can carry other user properties. Only its public fields
        // cross this channel, so an extra property cannot select our runtime calls.
        backgroundWorker.postMessage(isContext
            ? { type: message.type, data: message.data }
            : message);
    }
}
// Context events and runtime calls share the same FIFO, including calls the
// MTS entry makes before boot constructs its Worker. Keep references until
// postMessage performs the existing JSON snapshot.
jsContext.connect((event) => sendToBackground(event, true));
async function callLepusMethod(message) {
    try {
        const method = scope[message.name];
        const result = typeof method === "function"
            ? await method.call(scope, message.data)
            : undefined;
        if (message.id !== undefined) {
            sendToBackground({
                bobcat: "runtime", method: "callLepusMethodResult", id: message.id,
                result,
            });
        }
    }
    catch (error) {
        // Deliver failures to the calling worker even without a callback. Its
        // normal error path reports them; a success callback must not run.
        sendToBackground({
            bobcat: "runtime", method: "callLepusMethodResult", id: message.id,
            error: {
                name: error instanceof Error ? error.name : "Error",
                message: error instanceof Error ? error.message : String(error),
            },
        });
    }
}
/**
 * Called by boot only after the MTS entry finishes importing. Entry-level
 * listeners already exist; events it sent before Worker construction are
 * flushed in order through the same Worker transport as later events.
 */
export function __BobcatConnectBackground(worker) {
    worker.addEventListener("message", (event) => {
        const message = event.data;
        if (message?.bobcat === "runtime") {
            if (message.method === "callLepusMethod") {
                void callLepusMethod(message);
            }
        }
        else {
            jsContext.receive(message);
        }
    });
    backgroundWorker = worker;
    const queued = pendingBackgroundMessages;
    pendingBackgroundMessages = [];
    for (const { message, isContext } of queued) {
        sendToBackground(message, isContext);
    }
}
export function __BobcatPublishEvent(componentId, handlerName, event) {
    sendToBackground({
        bobcat: "runtime",
        method: componentId ? "publicComponentEvent" : "publishEvent",
        args: componentId ? [componentId, handlerName, event] : [handlerName, event],
    });
}
engineContext.addEventListener("__DestroyLifetime", () => {
    sendToBackground({ bobcat: "runtime", method: "callDestroyLifetimeFun" });
});
const globalEventEmitter = {
    addListener: noop,
    removeListener: noop,
    removeAllListeners: noop,
    emit: noop,
    trigger: noop,
    toggle: noop,
};
const runtimePerformance = {
    _generatePipelineOptions: noop,
    _onPipelineStart: noop,
    _bindPipelineIdWithTimingFlag: noop,
    _markTiming: noop,
    profileStart: noop,
    profileEnd: noop,
    profileMark: noop,
    profileFlowId: function () {
        return 0;
    },
    isProfileRecording: function () {
        return false;
    },
};
export const SystemInfo = Object.freeze({});
/**
 * Parses one piece of the host's page data: the string the view was given,
 * which nothing native reads, or `undefined` when it was given none — `{}`
 * here, as in web-core. So this is where malformed JSON is first met, and the
 * error names which input it was: `JSON.parse`'s own error places the fault
 * in an anonymous `<input>`.
 */
function parsePageData(name, json) {
    if (json === undefined) {
        return {};
    }
    try {
        return JSON.parse(json);
    }
    catch (error) {
        throw new SyntaxError(`${name} is not valid JSON: ${error.message}`);
    }
}
/** The host's init data, which boot hands to `processData`. */
export const __BobcatInitData = parsePageData("initData", initData());
export const __globalProps = parsePageData("globalProps", globalProps());
export function _AddEventListener() {
    return undefined;
}
export function _ReportError() {
    return undefined;
}
export function _SetSourceMapRelease() {
    return undefined;
}
export function __OnLifecycleEvent(data) {
    jsContext.dispatchEvent({ type: "__OnLifecycleEvent", data });
}
export const NativeModules = undefined;
export const lynx = {
    SystemInfo,
    __initData: {},
    __globalProps,
    performance: runtimePerformance,
    getCoreContext: function () {
        return coreContext;
    },
    getJSContext: function () {
        return jsContext;
    },
    getNative: function () {
        return nativeContext;
    },
    getEngine: function () {
        return engineContext;
    },
    getJSModule: function (name) {
        return name === "GlobalEventEmitter" ? globalEventEmitter : undefined;
    },
    registerDataProcessors: noop,
    reportError: _ReportError,
    triggerGlobalEventFromLepus: noop,
};
