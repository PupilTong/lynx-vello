// The compiled BTS module ABI: the factory ABI a bundle body defines modules
// through, and the one load every bundle path goes through.
//
// There is no table and no boot-time import here any more: `requireModule`,
// `loadScript` and `loadScriptInit` each build the URL their path names and
// load it synchronously, as `__LoadLepusChunk` does on MTS. What is stood in
// for is that one pair of host members, as in module.test.ts: the mock
// resolves with Node's own `URL`, refusing what the engine's normalizer
// refuses, and answers a body `PageSource` registered as the `"module"` it is
// — a namespace object — and a file nothing wrote into a bundle as the
// `"commonjs"` wrapper `new Function` builds. The real boundary, where the
// engine itself compiles and evaluates the module, runs in
// crates/bobcat-core/src/background/tests.rs.

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

/**
 * The whole parameter list a bundle path is asked for in. It is read only
 * where the response is a plain CommonJS file, which is what a path no
 * container carried normally is: none of the Lynx names a registered body
 * gets from its module's preamble is in scope there.
 */
const EXTERNAL_PARAMETERS = "module, exports";

/** One file the mock host serves, as one of the three shapes a load answers. */
type HostFile =
  | { kind: "commonjs"; text: string }
  | { kind: "json"; value: unknown }
  | { kind: "module"; namespace: object };

const files: Map<string, HostFile> = new Map();
/** Every URL a load was asked for, in order. */
const loads: string[] = [];
/** The `parameters` argument of every load, in order. */
const parameterLists: string[] = [];

/** A file nothing wrote into a bundle: plain CommonJS text. */
function commonjs(text: string): HostFile {
  return { kind: "commonjs", text };
}
/** A `.json` response, which the host parses rather than compiling. */
function json(value: unknown): HostFile {
  return { kind: "json", value };
}
/**
 * One body as `PageSource` registered it: an ES module whose **default
 * export** is what native's host would have kept as that script's completion
 * value — the compiler's `{init}` object, a CommonJS body's `module.exports`,
 * a JSON body's value.
 */
function body(value: unknown): HostFile {
  return { kind: "module", namespace: { default: value } };
}
/** A hand-written ES module, which need not export a `default` at all. */
function esModule(namespace: object): HostFile {
  return { kind: "module", namespace };
}

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

/**
 * The three shapes the engine answers a synchronous load with: the wrapper
 * function of a CommonJS file, whose `module.exports` is what it answers; the
 * parsed value of a JSON one; the namespace object of an ES module, which the
 * engine compiled, linked and evaluated before answering.
 */
function loadModuleSync(url: string, parameters: string): LoadedModuleSource {
  loads.push(url);
  parameterLists.push(parameters);
  const file = files.get(url);
  if (file === undefined) {
    throw new Error(`cannot load '${url}': no such file: ${url}`);
  }
  if (file.kind === "json") return { url, kind: "json", value: file.value };
  if (file.kind === "module") {
    return { url, kind: "module", value: file.namespace };
  }
  return {
    url,
    kind: "commonjs",
    value: new Function(...parameters.split(", "), file.text),
  };
}

type Values = Record<string, unknown>;

function environment(templateUrl?: string) {
  const app = { _apiList: { native: true } };
  const lynx = { SystemInfo: { platform: "headless" } };
  const modules = createLynxModules(app, lynx, console);
  Object.assign(app, modules);
  modules.registerTemplateUrl(templateUrl);
  return { app, modules };
}

/** The entry name a body's `{init}` is initialized under. */
function entryWhileInitializing(): unknown {
  return Reflect.get(globalThis, "globDynamicComponentEntry");
}

describe("one load per bundle path, and the factory ABI over it", () => {
  beforeEach(() => {
    files.clear();
    loads.length = 0;
    parameterLists.length = 0;
  });

  it("initializes a body's {init} default export with this realm's app object", () => {
    let seenEntry: unknown;
    const value = {
      init(this: unknown, inject: {tt: {define: Function; require: Function}}) {
        seenEntry = entryWhileInitializing();
        const owner = inject.tt;
        owner.define("late.js", function (
          this: unknown, _require: unknown, module: {exports: unknown},
          _exports: unknown, _Card: unknown, _setTimeout: unknown,
          _setInterval: unknown, _clearInterval: unknown, _clearTimeout: unknown,
          _NativeModules: unknown, api: unknown,
        ) {
          module.exports = {owner, receiver: this, api, holder: value};
        });
        return owner.require("late.js");
      },
    };
    const {app, modules} = environment("https://cdn.test/app/x.web.bundle");
    // A `.lynx.bundle` body: `PageSource` wrote `export default <the
    // compiler's own IIFE>`, so its default export is the `{init}` object.
    files.set("https://cdn.test/app/late.js", body(value));

    const result = modules.requireModule("/late.js");
    // lynx-core's `_$executeInit`: `init` is called on the object that carries
    // it, with this realm's one app object as `{tt}`.
    expect(result).toEqual({
      owner: app, receiver: app, api: app._apiList, holder: value,
    });
    expect(seenEntry).toBe("__Card__");
    expect(loads).toEqual(["https://cdn.test/app/late.js"]);
    // Published for the init alone.
    expect(Object.hasOwn(globalThis, "globDynamicComponentEntry")).toBe(false);
    // Cached under the bare path, as in lynx-core, so nothing is loaded
    // again, and the module the body defined is `require`able afterwards.
    expect(modules.requireModule("/late.js")).toBe(result);
    expect(loads).toEqual(["https://cdn.test/app/late.js"]);
    expect(modules.require("late.js")).toBe(result);
    expect(() => modules.require("absent.js")).toThrow("is not defined");
  });

  it("answers a body whose default export carries no init as it stands", () => {
    const exports = {answer: 42};
    const {modules} = environment("https://cdn.test/app/x.web.bundle");
    // A `.web.bundle` body: its module exported the `module.exports` its
    // CommonJS text left behind.
    files.set("https://cdn.test/app/raw.js", body(exports));
    files.set("https://cdn.test/app/data.json", json({message: "parsed by the host"}));
    files.set("https://cdn.test/app/nothing.js", body(undefined));

    expect(modules.requireModule("/raw.js")).toBe(exports);
    expect(modules.requireModule("/data.json")).toEqual({message: "parsed by the host"});
    // A body whose value is `undefined` answers that, and is asked for again
    // because nothing cacheable came back.
    expect(modules.requireModule("/nothing.js")).toBeUndefined();
    expect(loads).toEqual([
      "https://cdn.test/app/raw.js",
      "https://cdn.test/app/data.json",
      "https://cdn.test/app/nothing.js",
    ]);
    // Every load asks in the one parameter list, which only a CommonJS
    // response is compiled in.
    expect(parameterLists).toEqual(Array(3).fill(EXTERNAL_PARAMETERS));
  });

  it("answers an ES module with no default export with its namespace", () => {
    const {modules} = environment("https://cdn.test/app/x.web.bundle");
    files.set("https://cdn.test/app/hand-written.js",
      esModule({answer: 42, other: "kept"}));

    expect(modules.requireModule("/hand-written.js")).toEqual(
      {answer: 42, other: "kept"});
    expect(loads).toEqual(["https://cdn.test/app/hand-written.js"]);
  });

  it("roots a name before it resolves it, so either spelling is one URL", () => {
    const {modules} = environment("https://cdn.test/app/x.web.bundle");
    // A manifest path is written rooted and a section name is not, while a
    // caller may write either — native roots every path before it looks one
    // up (`js_app.cc` `App::LoadScript`), and `lynx.loadScript` is called
    // with bare section names.
    files.set("https://cdn.test/app/app-service.js", body({rooted: true}));
    files.set("https://cdn.test/app/background", body({section: true}));

    expect(modules.requireModule("/app-service.js")).toEqual({rooted: true});
    expect(modules.requireModule("app-service.js")).toEqual({rooted: true});
    expect(modules.loadScript("background", {})).toEqual({section: true});
    expect(modules.loadScript("/background", {})).toEqual({section: true});
    expect(loads).toEqual([
      "https://cdn.test/app/app-service.js",
      "https://cdn.test/app/app-service.js",
      "https://cdn.test/app/background",
      "https://cdn.test/app/background",
    ]);
  });

  it("answers a named section once per entry and per key", () => {
    const {app, modules} = environment("https://cdn.test/app/x.web.bundle");
    files.set("https://cdn.test/app/background",
      body({init: ({tt}: {tt: unknown}) => ({app: tt})}));

    const result = modules.loadScript("background", {});
    expect(result).toEqual({app});
    expect(modules.loadScript("background", {})).toBe(result);
    expect(loads).toEqual(["https://cdn.test/app/background"]);
    // A section is no part of `requireModule`'s own cache.
    expect(modules.requireModule("background")).not.toBe(result);
    // And a `bundleName` is an entry of its own, so its sections are too.
    files.set("https://lazy.test/bundle/background", body({lazy: true}));
    expect(modules.loadScript("background", {bundleName: "https://lazy.test/bundle/lazy.bundle"}))
      .toEqual({lazy: true});
    expect(modules.loadScript("background", {})).toBe(result);
  });

  it("loads a path no bundle carries as a plain CommonJS file", () => {
    const {modules} = environment("https://cdn.test/app/x.web.bundle");
    files.set("https://cdn.test/app/chunk.js", commonjs(
      "'use strict'; module.exports = {receiver: this, count: arguments.length,"
      + " lynx: typeof lynx, exportsAlias: exports};"));

    const result = modules.requireModule("/chunk.js");
    expect(loads).toEqual(["https://cdn.test/app/chunk.js"]);
    expect(parameterLists).toEqual([EXTERNAL_PARAMETERS]);
    expect(result.receiver).toBeUndefined();
    expect(result.count).toBe(2);
    // None of the Lynx names a registered body gets is in scope here: nothing
    // outside this engine wrote this file into a bundle.
    expect(result.lynx).toBe("undefined");

    expect(modules.requireModule("/chunk.js")).toBe(result);
    expect(loads).toEqual(["https://cdn.test/app/chunk.js"]);
  });

  it("resolves an entry that is itself an absolute URL beside itself", () => {
    const {modules} = environment("https://cdn.test/app/x.web.bundle");
    files.set("https://lazy.test/bundle/chunk.js", commonjs("exports.lazy = true;"));

    expect(modules.requireModule("/chunk.js", "https://lazy.test/bundle/lazy.bundle"))
      .toEqual({lazy: true});
    expect(loads).toEqual(["https://lazy.test/bundle/chunk.js"]);
  });

  it("asks for an absolute path as it is, whatever the template URL", () => {
    const {modules} = environment("https://cdn.test/app/x.web.bundle");
    files.set("https://other.test/a.js", commonjs("exports.absolute = true;"));

    expect(modules.requireModule("https://other.test/a.js")).toEqual({absolute: true});
    expect(loads).toEqual(["https://other.test/a.js"]);
  });

  it("answers a JSON response with the value the host parsed", () => {
    const {modules} = environment("https://cdn.test/app/x.web.bundle");
    files.set("https://cdn.test/app/data.json", json({value: 7}));

    expect(modules.requireModule("/data.json")).toEqual({value: 7});
    expect(loads).toEqual(["https://cdn.test/app/data.json"]);
  });

  it("publishes partial CommonJS exports for cycles and resolves relative names", () => {
    const {modules} = environment("https://cdn.test/app/x.web.bundle");
    files.set("https://cdn.test/app/dir.js", body({
      init({tt}: {tt: {define: Function}}) {
        tt.define("dir/a.js", function (require: Function, module: {exports: Values}) {
          module.exports["a"] = 1;
          module.exports["fromB"] = (require("./b") as Values)["fromA"];
        });
        tt.define("dir/b.js", function (require: Function, module: {exports: Values}) {
          module.exports["fromA"] = (require("../dir/a") as Values)["a"];
        });
      },
    }));

    modules.requireModule("/dir.js");
    expect(modules.require("dir/a.js")).toEqual({a: 1, fromB: 1});
  });

  it("throws a refused load through and caches nothing", () => {
    const {modules} = environment("https://cdn.test/app/x.web.bundle");

    expect(() => modules.requireModule("/absent.js")).toThrow(
      "cannot load 'https://cdn.test/app/absent.js'");
    files.set("https://cdn.test/app/absent.js", commonjs("exports.late = true;"));
    expect(modules.requireModule("/absent.js")).toEqual({late: true});
    expect(loads).toEqual([
      "https://cdn.test/app/absent.js", "https://cdn.test/app/absent.js"]);
  });

  it("retains no factory for a body whose init threw", () => {
    let runs = 0;
    const {modules} = environment("https://cdn.test/app/x.web.bundle");
    files.set("https://cdn.test/app/failing.js", body({
      init() {
        if (runs++ === 0) throw new Error("the first init failed");
        return {runs};
      },
    }));

    // Neither table is written until the factory has returned, so the next
    // call runs the whole path again — a second load, answered in the engine
    // from the module the realm has already evaluated.
    expect(() => modules.requireModule("/failing.js")).toThrow("the first init failed");
    expect(modules.requireModule("/failing.js")).toEqual({runs: 2});
    expect(loads).toEqual([
      "https://cdn.test/app/failing.js", "https://cdn.test/app/failing.js"]);
    expect(entryWhileInitializing()).toBeUndefined();
  });

  it("refuses a relative path with no template URL and still loads an absolute one", () => {
    const {modules} = environment();

    expect(() => modules.requireModule("/chunk.js")).toThrow(TypeError);
    expect(() => modules.requireModule("/chunk.js")).toThrow(
      "cannot resolve module './chunk.js'");
    expect(loads).toEqual([]);

    files.set("https://other.test/a.js", commonjs("exports.absolute = true;"));
    expect(modules.requireModule("https://other.test/a.js")).toEqual({absolute: true});
  });

  it("answers nativeApp.loadScript with an init that feeds no requireModule cache", () => {
    const {app, modules} = environment("https://cdn.test/app/x.web.bundle");
    files.set("https://cdn.test/app/lynx.js",
      body({init: ({tt}: {tt: unknown}) => ({api: tt})}));
    files.set("https://cdn.test/app/raw.js", commonjs("exports.raw = true;"));
    files.set("https://cdn.test/app/data.json", json({value: 7}));

    // The load is `loadScript`'s own, not `init`'s, as web-core's is.
    expect(modules.loadScriptInit("/lynx.js").init({tt: app}).api).toBe(app);
    expect(modules.loadScriptInit("/raw.js").init({tt: app})).toEqual({raw: true});
    expect(modules.loadScriptInit("/data.json").init({tt: app})).toEqual({value: 7});
    expect(loads).toEqual([
      "https://cdn.test/app/lynx.js",
      "https://cdn.test/app/raw.js",
      "https://cdn.test/app/data.json",
    ]);

    // Nothing of that reached `requireModule`'s tables, so this asks again.
    expect(modules.requireModule("/raw.js")).toEqual({raw: true});
    expect(loads).toEqual([
      "https://cdn.test/app/lynx.js",
      "https://cdn.test/app/raw.js",
      "https://cdn.test/app/data.json",
      "https://cdn.test/app/raw.js",
    ]);
  });
});
