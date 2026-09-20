// The compiled BTS module ABI: the registered-source tables, and the loader a
// path no manifest carries goes through.
//
// The two host members the loader is written over are stood in for here, as in
// module.test.ts: the mock resolves with Node's own `URL`, refusing what the
// engine's normalizer refuses, and compiles with `new Function`. The real
// boundary runs in crates/bobcat-core/src/background/tests.rs.

import { beforeEach, describe, expect, it, rstest } from "@rstest/core";
import { createLynxModules } from "../src/lynx-modules.ts";

// Only the functions this returns read the tables below, and they run when
// `lynx-modules.ts` calls them: the factory is hoisted above this file's
// bindings.
rstest.mockRequire("bobcat-internal:host", () => ({
  resolveModuleUrl: (base: string, specifier: string): string =>
    resolveModuleUrl(base, specifier),
  loadModuleSync: (url: string, parameters: string): LoadedModuleSource =>
    loadModuleSync(url, parameters),
}));

/** The parameter list web-core compiles an external chunk in. */
const LYNX_PARAMETERS =
  "postMessage, module, exports, lynxCoreInject, Card, setTimeout, setInterval, "
  + "clearInterval, clearTimeout, NativeModules, console, Component, ReactLynx, "
  + "nativeAppId, Behavior, LynxJSBI, lynx, window, document, frames, location, "
  + "navigator, localStorage, history, Caches, screen, alert, confirm, prompt, "
  + "webkit, Reporter, print, global, requestAnimationFrame, cancelAnimationFrame";

/** A Lynx-target chunk, as `RuntimeWrapperWebpackPlugin` banners one. */
const LYNX_TARGET_CHUNK = `(function(){'use strict';var g=globalThis;
  function __init_card_bundle__(lynxCoreInject){ var tt=lynxCoreInject.tt;
    tt.define("/a.js", function(require,module){ module.exports={api:tt}; });
    return tt.require("/a.js"); }
  if (g.bundleSupportLoadScript){ var res={init:__init_card_bundle__};
    g.__bundle__holder=res; return res; }
  __init_card_bundle__({tt:tt}); })();`;

/**
 * A Lynx-target chunk whose module body throws the first time it runs and
 * answers the second, counting its runs where only `globalThis` can carry a
 * value a compiled chunk reaches.
 */
const FAILING_LYNX_CHUNK = `(function(){'use strict';var g=globalThis;
  function __init_card_bundle__(lynxCoreInject){ var tt=lynxCoreInject.tt;
    tt.define("/failing.js", function(require,module){
      if (g.__initRuns__++ === 0) throw new Error("the first init failed");
      module.exports={runs:g.__initRuns__}; });
    return tt.require("/failing.js"); }
  if (g.bundleSupportLoadScript){ var res={init:__init_card_bundle__};
    g.__bundle__holder=res; return res; }
  __init_card_bundle__({tt:tt}); })();`;

/** One file the mock host serves. */
interface HostFile {
  text: string;
  kind?: "commonjs" | "json";
}

const files: Map<string, HostFile> = new Map();
/** Every URL a load was asked for, in order. */
const loads: string[] = [];
/** The `parameters` argument of every load, in order. */
const parameterLists: string[] = [];

/** As strict as the engine's normalizer, and as plain in how it says so. */
function resolveModuleUrl(base: string, specifier: string): string {
  if (typeof base !== "string") {
    throw new Error("resolveModuleUrl expects a string for argument 0");
  }
  const absolute = parsed(specifier);
  if (absolute) return absolute.href;
  if (!/^\.{0,2}\//.test(specifier)) {
    throw new Error(`bare module specifier '${specifier}' is not supported`);
  }
  const joined = parsed(specifier, base);
  if (joined === undefined) {
    throw new Error(`cannot resolve module '${specifier}' from '${base}'`);
  }
  return joined.href;
}

/** `specifier` as a URL against `base`, or nothing when it is not one. */
function parsed(specifier: string, base?: string): URL | undefined {
  try {
    return new URL(specifier, base);
  } catch {
    return undefined;
  }
}

function loadModuleSync(url: string, parameters: string): LoadedModuleSource {
  loads.push(url);
  parameterLists.push(parameters);
  const file = files.get(url);
  if (file === undefined) {
    throw new Error(`cannot load '${url}': no such file: ${url}`);
  }
  if (file.kind === "json") {
    return { url, kind: "json", value: JSON.parse(file.text) };
  }
  return {
    url,
    kind: "commonjs",
    value: new Function(...parameters.split(", "), file.text),
  };
}

function environment(templateUrl?: string) {
  const app = { _apiList: { native: true } };
  const lynx = { SystemInfo: { platform: "headless" } };
  const modules = createLynxModules(app, lynx, console);
  Object.assign(app, modules);
  modules.registerTemplateUrl(templateUrl);
  return { app, modules };
}

/** What a chunk left in the holder, which only `globalThis` can carry. */
function holder(): unknown {
  return Reflect.get(globalThis, "__bundle__holder");
}

describe("compiled BTS module ABI", () => {
  it("gives definitions an app scope and factories the compiler API arguments", () => {
    const { app, modules } = environment();
    modules.register({ '/late.js': `
      const owner=tt, helper=41;
      tt.define('late.js',function(require,module,exports,Card,setTimeout,setInterval,clearInterval,clearTimeout,NativeModules,tt){
        module.exports={owner,receiver:this,api:tt,value:helper+1,strict:(function(){return this})()===undefined};
      });
      ({init(){throw Error('tt.require must ignore Script completion');}})
    ` }, true);
    const result = modules.require('late.js');
    expect(result).toEqual({owner:app,receiver:app,api:app._apiList,value:42,strict:true});
    expect(modules.require('late.js')).toBe(result);
  });

  it("evaluates native init wrappers and converted web copies with the same app", () => {
    const source = `(function(){return {init:function({tt}){
      tt.define('/entry.js',function(require,module,exports,Card,setTimeout,setInterval,clearInterval,clearTimeout,NativeModules,tt,console,Component,ReactLynx,nativeAppId,Behavior,LynxJSBI,lynx){
        module.exports={argc:arguments.length,api:tt,platform:lynx.SystemInfo.platform,receiver:this};
      });
      return tt.require('/entry.js');
    }}})()`;
    for (const wrapped of [true, false]) {
      const {app, modules} = environment();
      modules.register({'/entry.js': source}, wrapped);
      const result = modules.requireModule('/entry.js');
      expect(result).toEqual({argc:39,api:app._apiList,platform:"headless",receiver:app});
      expect(modules.requireModule('/entry.js')).toBe(result);
    }
  });

  it("allows lexical minifier names in web Scripts without leaking bindings", () => {
    const {modules} = environment();
    modules.register({'/web.js': '"use strict"; let tt=42; module.exports={tt,platform:lynx.SystemInfo.platform,scope:lynxCoreInject.tt._apiList.native};'}, false);
    expect(modules.requireModule('/web.js')).toEqual({tt:42,platform:"headless",scope:true});
    expect(Object.hasOwn(globalThis, 'lynxCoreInject')).toBe(false);
  });

  it("publishes partial CommonJS exports for cycles and resolves relative names", () => {
    const {modules} = environment();
    modules.register({
      '/dir/a.js': `tt.define('dir/a.js',function(require,module){
        module.exports.a=1; module.exports.fromB=require('./b').fromA;
      });`,
      '/dir/b.js': `tt.define('dir/b.js',function(require,module){
        module.exports.fromA=require('../dir/a').a;
      });`,
    }, false);
    expect(modules.require('dir/a.js')).toEqual({a:1,fromB:1});
  });

  it("loads registered JSON and named sections and rejects absent sources", () => {
    const {app, modules} = environment();
    modules.register({'/data.json':'{"value":7}'}, false);
    expect(modules.requireModule('/data.json')).toEqual({value:7});
    modules.registerSections({background:'({init({tt}){return {app:tt}}})'});
    const result = modules.loadScript('background', {});
    expect(result).toEqual({app});
    expect(modules.loadScript('background', {})).toBe(result);
    expect(() => modules.loadScript('absent', {})).toThrow('not registered');
  });
});

describe("requireModule over the synchronous host loader", () => {
  beforeEach(() => {
    files.clear();
    loads.length = 0;
    parameterLists.length = 0;
    Reflect.set(globalThis, "__bundle__holder", undefined);
  });

  it("resolves an unregistered path against the template URL and runs its body once", () => {
    const {app, modules} = environment("https://cdn.test/app/x.web.bundle");
    files.set("https://cdn.test/app/chunk.js", {text:
      "'use strict'; module.exports = {receiver: this, count: arguments.length,"
      + " api: lynxCoreInject.tt, entry: globalThis.globDynamicComponentEntry,"
      + " bom: typeof window, raf: requestAnimationFrame};"});

    const result = modules.requireModule('/chunk.js');
    expect(loads).toEqual(["https://cdn.test/app/chunk.js"]);
    expect(parameterLists).toEqual([LYNX_PARAMETERS]);
    expect(result.receiver).toBeUndefined();
    expect(result.count).toBe(35);
    expect(result.api).toBe(app);
    expect(result.entry).toBe("__Card__");
    expect(result.bom).toBe("undefined");
    expect(result.raf).toBeUndefined();
    // The entry is published for the body alone, and the holder is left clear.
    expect(Object.hasOwn(globalThis, 'globDynamicComponentEntry')).toBe(false);
    expect(holder()).toBeUndefined();

    expect(modules.requireModule('/chunk.js')).toBe(result);
    expect(loads).toEqual(["https://cdn.test/app/chunk.js"]);
  });

  it("roots a bare path before it resolves it", () => {
    const {modules} = environment("https://cdn.test/app/x.web.bundle");
    files.set("https://cdn.test/app/chunk.js", {text: "exports.rooted = true;"});

    expect(modules.requireModule('chunk.js')).toEqual({rooted: true});
    expect(loads).toEqual(["https://cdn.test/app/chunk.js"]);
  });

  it("asks for an absolute path as it is, whatever the template URL", () => {
    const {modules} = environment("https://cdn.test/app/x.web.bundle");
    files.set("https://other.test/a.js", {text: "exports.absolute = true;"});

    expect(modules.requireModule('https://other.test/a.js')).toEqual({absolute: true});
    expect(loads).toEqual(["https://other.test/a.js"]);
  });

  it("takes a Lynx-target chunk's factory out of the bundle holder", () => {
    const {app, modules} = environment("https://cdn.test/app/x.web.bundle");
    files.set("https://cdn.test/app/lynx.js", {text: LYNX_TARGET_CHUNK});

    expect(modules.requireModule('/lynx.js').api).toBe(app);
    expect(holder()).toBeUndefined();
  });

  it("answers a JSON response with the value the host parsed", () => {
    const {modules} = environment("https://cdn.test/app/x.web.bundle");
    files.set("https://cdn.test/app/data.json", {text: '{"value": 7}', kind: "json"});

    expect(modules.requireModule('/data.json')).toEqual({value: 7});
    expect(loads).toEqual(["https://cdn.test/app/data.json"]);
  });

  it("throws a refused load through and caches nothing", () => {
    const {modules} = environment("https://cdn.test/app/x.web.bundle");

    expect(() => modules.requireModule('/absent.js')).toThrow(
      "cannot load 'https://cdn.test/app/absent.js'");
    files.set("https://cdn.test/app/absent.js", {text: "exports.late = true;"});
    expect(modules.requireModule('/absent.js')).toEqual({late: true});
    expect(loads).toEqual([
      "https://cdn.test/app/absent.js", "https://cdn.test/app/absent.js"]);
  });

  it("retains no factory for a chunk whose init threw and loads the file again", () => {
    const {modules} = environment("https://cdn.test/app/x.web.bundle");
    Reflect.set(globalThis, "__initRuns__", 0);
    files.set("https://cdn.test/app/failing.js", {text: FAILING_LYNX_CHUNK});

    expect(() => modules.requireModule('/failing.js')).toThrow("the first init failed");
    expect(modules.requireModule('/failing.js')).toEqual({runs: 2});
    expect(loads).toEqual([
      "https://cdn.test/app/failing.js", "https://cdn.test/app/failing.js"]);
  });

  it("refuses a relative path with no template URL and still loads an absolute one", () => {
    const {modules} = environment();

    expect(() => modules.requireModule('/chunk.js')).toThrow(TypeError);
    expect(() => modules.requireModule('/chunk.js')).toThrow(
      "cannot resolve module './chunk.js'");
    expect(loads).toEqual([]);

    files.set("https://other.test/a.js", {text: "exports.absolute = true;"});
    expect(modules.requireModule('https://other.test/a.js')).toEqual({absolute: true});
  });

  it("never reaches the loader for a registered path", () => {
    const {modules} = environment("https://cdn.test/app/x.web.bundle");
    modules.register({'/entry.js': 'module.exports = {registered: true};'}, false);
    files.set("https://cdn.test/app/entry.js", {text: "exports.loaded = true;"});

    expect(modules.requireModule('/entry.js')).toEqual({registered: true});
    expect(loads).toEqual([]);
  });

  it("answers nativeApp.loadScript with an init that feeds no requireModule cache", () => {
    const {app, modules} = environment("https://cdn.test/app/x.web.bundle");
    files.set("https://cdn.test/app/lynx.js", {text: LYNX_TARGET_CHUNK});
    files.set("https://cdn.test/app/raw.js", {text: "exports.raw = true;"});
    files.set("https://cdn.test/app/data.json", {text: '{"value": 7}', kind: "json"});

    expect(modules.loadScriptInit('/lynx.js').init({tt: app}).api).toBe(app);
    expect(modules.loadScriptInit('/raw.js').init({tt: app})).toEqual({raw: true});
    expect(modules.loadScriptInit('/data.json').init({tt: app})).toEqual({value: 7});
    expect(loads).toEqual([
      "https://cdn.test/app/lynx.js",
      "https://cdn.test/app/raw.js",
      "https://cdn.test/app/data.json",
    ]);

    // Nothing of that reached `requireModule`'s tables, so this loads again.
    expect(modules.requireModule('/raw.js')).toEqual({raw: true});
    expect(loads).toEqual([
      "https://cdn.test/app/lynx.js",
      "https://cdn.test/app/raw.js",
      "https://cdn.test/app/data.json",
      "https://cdn.test/app/raw.js",
    ]);
  });

  it("serves a registered source from nativeApp.loadScript too", () => {
    const {app, modules} = environment("https://cdn.test/app/x.web.bundle");
    modules.register({'/entry.js': '({init({tt}){return {app:tt}}})'}, true);

    expect(modules.loadScriptInit('/entry.js').init({tt: app})).toEqual({app});
    expect(loads).toEqual([]);
  });
});
