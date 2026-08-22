// The shape-only `bobcat:runtime` ESM loaded before an MTS entry runs.
//
// Bobcat does not have the background-thread realm, cross-context transport,
// native-module registry, error reporter, or lifecycle delivery path yet.  A
// compiled main-thread chunk still probes those APIs while it installs the
// ReactLynx snapshot runtime, so this file supplies explicit sinks for that
// bootstrap surface.  They intentionally keep no listeners, deliver no
// messages, expose no native modules, and report no lifecycle events.
//
// This is not an Element PAPI implementation.  Every `__*` element member,
// including the scoped-style sink `__SetCSSId`, belongs to element-papi.js,
// which the host evaluates next.  Background-thread-only globals such as
// `lynxCoreInject` also do not belong in this realm.

(function () {
  "use strict";

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
  const jsContext = createContextSink();
  const nativeContext = createContextSink();

  const globalEventEmitter = {
    addListener: noop,
    removeListener: noop,
    removeAllListeners: noop,
    emit: noop,
    trigger: noop,
    toggle: noop,
  };

  const performance = {
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

  const systemInfo = Object.freeze({});
  const initData = {};
  const globalProps = {};

  const lynx = {
    SystemInfo: systemInfo,
    __initData: initData,
    __globalProps: globalProps,
    performance: performance,
    getCoreContext: function () {
      return coreContext;
    },
    getJSContext: function () {
      return jsContext;
    },
    getNative: function () {
      return nativeContext;
    },
    getJSModule: function (name) {
      return name === "GlobalEventEmitter" ? globalEventEmitter : undefined;
    },
    registerDataProcessors: noop,
    reportError: noop,
    triggerGlobalEventFromLepus: noop,
  };

  Object.assign(globalThis, {
    lynx: lynx,
    SystemInfo: systemInfo,
    __globalProps: globalProps,
    // ReactLynx deliberately installs this main-thread sentinel before any
    // eager probe; the actual native-module table belongs to the JS thread.
    NativeModules: undefined,
    _AddEventListener: noop,
    _ReportError: noop,
    _SetSourceMapRelease: noop,
    __OnLifecycleEvent: noop,
  });
})();

const {
  lynx,
  SystemInfo,
  __globalProps,
  NativeModules,
  _AddEventListener,
  _ReportError,
  _SetSourceMapRelease,
  __OnLifecycleEvent,
} = globalThis;

export {
  lynx,
  SystemInfo,
  __globalProps,
  NativeModules,
  _AddEventListener,
  _ReportError,
  _SetSourceMapRelease,
  __OnLifecycleEvent,
};
