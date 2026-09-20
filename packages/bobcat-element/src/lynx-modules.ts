import { loadModuleSync, resolveModuleUrl } from "bobcat-internal:host";

// The compiler factory ABI over the sources `PageSource` registered, and the
// synchronous loader for a path no registration carries. Lazy containers —
// `requireModuleAsync`, `fetchBundle` and the lazy-bundle `loadScript` — are
// handled separately from this table and are not here.
const DEFAULT_ENTRY = "__Card__";
const FACTORY_ARGUMENTS = [
  "require", "module", "exports", "Card", "setTimeout", "setInterval",
  "clearInterval", "clearTimeout", "NativeModules", "tt", "console",
  "Component", "ReactLynx", "nativeAppId", "Behavior", "LynxJSBI", "lynx",
  "window", "document", "frames", "self", "location", "navigator",
  "localStorage", "history", "Caches", "screen", "alert", "confirm", "prompt",
  "fetch", "XMLHttpRequest", "__WebSocket__", "webkit", "Reporter", "print",
  "global", "requestAnimationFrame", "cancelAnimationFrame",
];

// The parameter list an externally loaded chunk is compiled in: web-core's,
// from `createBundleInitReturnObj`
// (web-core/ts/client/background/background-apis/createChunkLoading.ts).
// `Card` and `Component` are always among them, where web-core drops the pair
// for a React card: a chunk that does not name them is unaffected either way,
// and the registered-source path passes `app.Card`/`app.Component` already.
const LYNX_PARAMETERS = [
  "postMessage", "module", "exports", "lynxCoreInject", "Card", "setTimeout",
  "setInterval", "clearInterval", "clearTimeout", "NativeModules", "console",
  "Component", "ReactLynx", "nativeAppId", "Behavior", "LynxJSBI", "lynx",
  "window", "document", "frames", "location", "navigator", "localStorage",
  "history", "Caches", "screen", "alert", "confirm", "prompt", "webkit",
  "Reporter", "print", "global", "requestAnimationFrame",
  "cancelAnimationFrame",
].join(", ");

/** A specifier that carries its own scheme, and so needs no base at all. */
const ABSOLUTE_URL = /^[A-Za-z][A-Za-z\d+\-.]*:/;

// Compiler modules choose their own export shape and replace module.exports.
// This boundary deliberately carries those JS values without a runtime schema.
type ModuleValue = any;
interface Definition { factory: Function; hasRun: boolean; exports?: ModuleValue }
interface Source { source: string; wrapped: boolean }

export function createLynxModules(app: ModuleValue, lynx: ModuleValue, console: ModuleValue) {
  const globals = globalThis as unknown as {
    bundleSupportLoadScript: boolean;
    globDynamicComponentEntry?: string;
    __bundle__holder: {init?: unknown} | undefined;
  };
  globals.bundleSupportLoadScript = true;
  const definitions = new Map<string, Map<string, Definition>>();
  const sources = new Map<string, Map<string, Source>>();
  const sections = new Map<string, Record<string, string>>();
  const cache = new Map<string, ModuleValue>();
  const factories = new Map<string, () => ModuleValue>();
  const sectionExports = new Map<string, ModuleValue>();
  /** The URL each entry's own template answered from, as its resolution base. */
  const templateUrls = new Map<string, string>();

  function table<T>(registry: Map<string, Map<string, T>>, entry: string): Map<string, T> {
    let result = registry.get(entry);
    if (!result) registry.set(entry, result = new Map());
    return result;
  }
  function define(path: string, factory: Function, entry = DEFAULT_ENTRY) {
    table(definitions, entry || DEFAULT_ENTRY).set(path, { factory, hasRun: false });
  }
  function findSource(path: string, entry: string): Source | undefined {
    const source = table(sources, entry);
    return source.get(path) ?? source.get(path.startsWith("/") ? path : `/${path}`);
  }
  function relativeRequireFor(path: string) {
    return (requested: string): ModuleValue => {
      const directory = path.match(/(.*)\/([^/]+)?$/)?.[1] || "./";
      const parts: string[] = [];
      for (const part of `${directory}/${requested}`.split("/")) {
        if (part === "..") {
          if (!parts.length) throw new Error(`can't find module ${requested}`);
          parts.pop();
        } else if (part && part !== ".") parts.push(part);
      }
      let normalized = parts.join("/");
      if (!normalized.endsWith(".js")) normalized += ".js";
      return require(normalized);
    };
  }
  function argumentsFor(path: string, module: {exports: ModuleValue}, params?: ModuleValue) {
    return [relativeRequireFor(path), module, module.exports,
      app.Card, lynx.setTimeout, lynx.setInterval, lynx.clearInterval, lynx.clearTimeout,
      app.NativeModules, app._apiList, console, app.Component, params?.ReactLynx,
      app.nativeAppId, app.Behavior, app.LynxJSBI, lynx, ...Array(13).fill(undefined),
      lynx.fetch, ...Array(6).fill(undefined), lynx.requestAnimationFrame, lynx.cancelAnimationFrame];
  }
  function require(path: string, params?: ModuleValue): ModuleValue {
    const entry: string = params?.dynamicComponentEntry || DEFAULT_ENTRY;
    const modules = table(definitions, entry);
    let record = modules.get(path);
    if (!record) {
      const source = findSource(path, entry);
      if (!source) throw new Error(`module ${path} in ${entry} is not registered`);
      const evaluate = new Function("lynx", "SystemInfo", "console", "source",
        '"use strict"; const tt=this; return eval(source);');
      evaluate.call(app, lynx, lynx.SystemInfo, console, source.source);
      record = modules.get(path);
      if (!record) throw new Error(`module ${path} in ${entry} is not defined`);
    }
    if (!record.hasRun) {
      const module = {exports: {}};
      record.hasRun = true;
      record.exports = module.exports;
      const result = record.factory.apply(app, argumentsFor(path, module, params));
      record.exports = module.exports || result;
    }
    return record.exports;
  }

  /**
   * The URL an unregistered `path` names: the path as a reference beside the
   * template the bundle came in, resolved by the normalizer an `import` there
   * resolves through.
   *
   * The rooting is native's (`js_app.cc` `App::LoadScript`): a path that is
   * neither an absolute URL nor rooted is rooted, so `chunk.js` and
   * `/chunk.js` name one file. A rooted path is a path *inside* a bundle, so
   * it resolves beside its template rather than at that template's origin.
   * With no template URL registered the reference is its own base, which
   * resolves an absolute URL and refuses everything else — there is nothing
   * for a bundle path to be a path inside.
   */
  function externalUrl(path: string, entry: string): string {
    const specifier = ABSOLUTE_URL.test(path)
      ? path : `.${path.startsWith("/") ? path : `/${path}`}`;
    const base = templateUrls.get(entry) ?? templateUrls.get(DEFAULT_ENTRY) ?? specifier;
    try {
      return resolveModuleUrl(base, specifier);
    } catch (error) {
      // Every host member reports a failure as an `InternalError`, so which
      // class a path this engine cannot resolve reaches author code as is this
      // realm's to choose, as it is in `bobcat:module`.
      throw new TypeError(error instanceof Error ? error.message : String(error));
    }
  }
  /**
   * One value per name in `LYNX_PARAMETERS`, in that order.
   *
   * `postMessage`, `ReactLynx` and the whole BOM block are `undefined`: this
   * realm has no value for them, and a chunk that reads one gets `undefined`
   * rather than whatever the global scope happens to carry.
   */
  function chunkArgumentsFor(module: {exports: ModuleValue}) {
    return [undefined, module, module.exports, {tt: app}, app.Card,
      lynx.setTimeout, lynx.setInterval, lynx.clearInterval, lynx.clearTimeout,
      app.NativeModules, console, app.Component, undefined, app.nativeAppId,
      app.Behavior, app.LynxJSBI, lynx, ...Array(16).fill(undefined),
      lynx.requestAnimationFrame, lynx.cancelAnimationFrame];
  }
  /** Whatever a chunk left in the holder, and the holder left clear. */
  function takeHolder(): {init?: unknown} | undefined {
    const holder = globals.__bundle__holder;
    globals.__bundle__holder = undefined;
    return holder;
  }
  /**
   * One path no registration carries, loaded and run, as the factory of its
   * exports.
   *
   * The load is synchronous and parks the job this call runs in;
   * `loadModuleSync` compiles the source in `LYNX_PARAMETERS` — the text
   * never becomes a value here — or parses it as JSON, which the *response*
   * URL's own extension decides.
   */
  function loadExternal(path: string, entry: string): () => ModuleValue {
    const loaded = loadModuleSync(externalUrl(path, entry), LYNX_PARAMETERS);
    if (loaded.kind === "json") {
      const parsed = loaded.value;
      return () => parsed;
    }
    const module = {exports: {} as ModuleValue};
    const previousEntry = globals.globDynamicComponentEntry;
    globals.globDynamicComponentEntry = entry;
    // A Lynx-target chunk answers through `globalThis.__bundle__holder`: its
    // banner stores `{init}` there because `bundleSupportLoadScript` is set, a
    // wrapper function having no reachable completion value
    // (`RuntimeWrapperWebpackPlugin`'s `loadScriptFooter`). A raw CommonJS body
    // leaves the holder alone and answers through `module.exports`.
    globals.__bundle__holder = undefined;
    let holder: {init?: unknown} | undefined;
    try {
      (loaded.value as Function).apply(undefined, chunkArgumentsFor(module));
    } finally {
      holder = takeHolder();
      if (previousEntry === undefined) delete globals.globDynamicComponentEntry;
      else globals.globDynamicComponentEntry = previousEntry;
    }
    const init = holder?.init;
    if (typeof init === "function") return () => init.call(holder, {tt: app});
    const value = module.exports;
    return () => value;
  }
  /** One registered source, evaluated, as the factory of its exports. */
  function registeredFactory(path: string, resource: Source, entry: string): () => ModuleValue {
    if (path.split("?")[0]?.endsWith(".json")) {
      const value: unknown = JSON.parse(resource.source);
      return () => value;
    }
    const previousEntry = globals.globDynamicComponentEntry;
    globals.globDynamicComponentEntry = entry;
    try {
      // Direct eval gives each compiler Script its own lexical scope and
      // retains its completion value without changing the native evaluator.
      const evaluate = new Function(...FACTORY_ARGUMENTS,
        "lynxCoreInject", "SystemInfo", "globDynamicComponentEntry", "source", "return eval(source);");
      const module = {exports: {}};
      const bundle = evaluate.apply(app, [...argumentsFor(path, module),
        {tt: app}, lynx.SystemInfo, entry, resource.source]);
      if (resource.wrapped || typeof bundle?.init === "function") {
        if (typeof bundle?.init !== "function") throw new Error(`bundle ${path} has no init factory`);
        return () => bundle.init({tt: app});
      }
      const value = module.exports || bundle;
      return () => value;
    } finally {
      if (previousEntry === undefined) delete globals.globDynamicComponentEntry;
      else globals.globDynamicComponentEntry = previousEntry;
    }
  }
  /**
   * `lynx.requireModule`: the exports of one bundle path, from the registered
   * sources or from a load, evaluated once per realm.
   *
   * The key of both tables is the bare `path`, the entry no part of it, as in
   * lynx-core (`app.ts` `_$factoryCache`). Nothing is cached until the load,
   * the compile and the body have all returned, so a path that threw is loaded
   * again by the next call.
   *
   * `options` is accepted and ignored. web-core has no fetch timeout at all,
   * and native's is unreachable from here: its `loadScript` binding reads a
   * timeout only from a *number* third argument (`js_app.cc:197-199`), where
   * lynx-core hands it the whole `options` object, so native's own default
   * stands too.
   */
  function requireModule(path: string, entry = DEFAULT_ENTRY,
    _options?: {timeout?: number}): ModuleValue {
    const cached = cache.get(path);
    if (cached) return cached;
    let factory = factories.get(path);
    if (!factory) {
      const resource = findSource(path, entry);
      factory = resource ? registeredFactory(path, resource, entry) : loadExternal(path, entry);
    }
    // Both tables are written after the body returned, as lynx-core writes
    // `_$factoryCache` only there (`app.ts` `loadScript`): a factory whose
    // `init` threw is not retained either, so the next call runs the whole
    // path — load, compile, body — again.
    const value = factory();
    factories.set(path, factory);
    cache.set(path, value);
    return value;
  }
  /**
   * `nativeApp.loadScript(sourceURL, entryName)`: the `{init}` object web-core
   * answers with (`createChunkLoading.ts` `createBundleInitReturnObj`), whose
   * `init` answers the module's exports — the chunk's own `init({tt})` for a
   * Lynx-target file, `module.exports` for a raw body, the parsed value for
   * JSON.
   *
   * The registered sources first, then a load, and neither of
   * `requireModule`'s two tables is written: lynx-core's `loadScript` feeds
   * neither either, so a `requireModule` of the same path afterwards loads it
   * again.
   *
   * `init` ignores the injection it is handed. This realm has one app object,
   * it is the one every factory here is given, and a raw body has already run
   * with it by the time `init` is callable.
   */
  function loadScriptInit(sourceURL: string, entry = DEFAULT_ENTRY) {
    const resource = findSource(sourceURL, entry);
    const factory = resource ? registeredFactory(sourceURL, resource, entry)
      : loadExternal(sourceURL, entry);
    return {init: (_inject?: ModuleValue): ModuleValue => factory()};
  }
  function loadScript(key: string, options: {bundleName?: string}): ModuleValue {
    const entry = options.bundleName ?? DEFAULT_ENTRY;
    const cacheKey = `${entry}:${key}`;
    if (sectionExports.has(cacheKey)) return sectionExports.get(cacheKey);
    const source = sections.get(entry)?.[key];
    if (source === undefined) throw new Error(`bundle section ${key} in ${entry} is not registered`);
    const evaluate = new Function("lynx", "lynxCoreInject", "SystemInfo", "console", "source", "return eval(source);");
    const factory = evaluate.call(globalThis, lynx, {tt: app}, lynx.SystemInfo, console, source);
    const result = typeof factory?.init === "function" ? factory.init({tt: app}) : factory;
    sectionExports.set(cacheKey, result);
    return result;
  }
  function register(manifest: Record<string, string>, wrapped: boolean, entry = DEFAULT_ENTRY) {
    const target = table(sources, entry);
    for (const [path, source] of Object.entries(manifest)) target.set(path, {source, wrapped});
  }
  function registerSections(values: Record<string, string>, entry = DEFAULT_ENTRY) {
    sections.set(entry, values);
  }
  /** No URL is no base: an entry registered without one resolves nothing. */
  function registerTemplateUrl(url: string | undefined, entry = DEFAULT_ENTRY) {
    if (url !== undefined) templateUrls.set(entry, url);
  }
  return {define, require, requireModule, loadScript, loadScriptInit, register,
    registerSections, registerTemplateUrl};
}
