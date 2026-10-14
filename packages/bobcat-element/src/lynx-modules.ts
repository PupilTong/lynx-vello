import { loadModuleSync, resolveModuleUrl } from "bobcat-internal:host";
import { bundlePathURL, sectionURL } from "bobcat:section-url";

// The compiler factory ABI, and the one way a bundle path becomes a value.
//
// One mechanism, and it is the mechanism MTS's `__LoadLepusChunk` uses: a path
// is a URL this realm builds, and `loadModuleSync` is what loads it, before the
// call returns. There is no table of bodies and no boot-time import loop —
// `requireModule`, `loadScript` and `loadScriptInit` all go through `loadBody`,
// which is what lets them stay synchronous without any source text reaching
// this realm.
//
// What is at that URL is `bobcat-source`'s business. A body a container
// carried is an ES module it registered *under* the container's URL — the
// compiler's `{init}` expression as a default export, a `CommonJS` file's
// `module.exports` as one, a `.json` body as the value it is — so a `require`
// of it answers that default export. A path no container carried is whatever
// the host serves, normally a plain `CommonJS` file.
//
// Every container is named by one rule, the page's own and a lazy one alike
// (`section-url.ts`, `crates/bobcat-source/src/container.rs`): a manifest path
// as the path it is under the container URL, a section name as an encoded
// `.js` file under it. Only *which* URL is the container differs — the one
// `__BobcatRegisterBundle` named for an entry, or a lazy container's
// `bundleName` itself. The container is installed by whoever fetched it;
// nothing here fetches one. `requireModuleAsync` is still absent.
const DEFAULT_ENTRY = "__Card__";

/** A specifier that carries its own scheme, and so needs no base at all. */
const ABSOLUTE_URL = /^[A-Za-z][A-Za-z\d+\-.]*:/;

// Compiler modules choose their own export shape and replace module.exports.
// This boundary deliberately carries those JS values without a runtime schema.
type ModuleValue = any;
interface Definition { factory: Function; hasRun: boolean; exports?: ModuleValue }

export function createLynxModules(app: ModuleValue, lynx: ModuleValue, console: ModuleValue) {
  const globals = globalThis as unknown as {
    bundleSupportLoadScript: boolean;
    globDynamicComponentEntry?: string;
  };
  // Read by a `RuntimeWrapperWebpackPlugin` banner as it evaluates: with it
  // set the banner *answers* with its `{init}` object rather than initializing
  // the card itself, which is the value the banner's module then exports.
  globals.bundleSupportLoadScript = true;
  const definitions = new Map<string, Map<string, Definition>>();
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
    // Only what a chunk's own body has defined: a bundle path is a *load* now,
    // and the body that ran is what calls `tt.define`.
    const record = modules.get(path);
    if (!record) throw new Error(`module ${path} in ${entry} is not defined`);
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
   * The container URL `entry` names, or `undefined` for none.
   *
   * **A registered entry** — this page's own, or any other container whose
   * URL `__BobcatRegisterBundle` named — is that URL. **Any other entry is a
   * lazy container's `bundleName`** — the string `lynx.fetchBundle` was given
   * and echoed back — and is its own container URL, left as it is, rooted or
   * relative: the host resolves the load against the view's base URL, the
   * fetcher resolved the container's fetch against its own, and the embedder
   * keeps the two bases equal, so both name one URL. The default entry with
   * no URL registered names no container at all.
   */
  function containerURL(entry: string): string | undefined {
    return templateUrls.get(entry) ?? (entry === DEFAULT_ENTRY ? undefined : entry);
  }
  /** The container `entry` names, or a `TypeError` naming what needed one. */
  function requireContainer(name: string, entry: string): string {
    const container = containerURL(entry);
    if (container === undefined) {
      throw new TypeError(`cannot resolve '${name}': no container URL is registered for '${entry}'`);
    }
    return container;
  }
  /**
   * The URL one bundle path names: an absolute URL as it is, and anything
   * else as a manifest path under `entry`'s container, the string
   * `bundlePathURL` writes — one leading `/` stripped, so `chunk.js` and
   * `/chunk.js` name one file, and a rooted path is a path *inside* the
   * container rather than one at its origin (native's rooting, `js_app.cc`
   * `App::LoadScript`).
   */
  function pathURL(path: string, entry: string): string {
    if (!ABSOLUTE_URL.test(path)) return bundlePathURL(path, requireContainer(path, entry));
    try {
      return resolveModuleUrl(path, path);
    } catch (error) {
      // Every host member reports a failure as an `InternalError`, so which
      // class a path this engine cannot resolve reaches author code as is this
      // realm's to choose, as it is in `bobcat:module`.
      throw new TypeError(error instanceof Error ? error.message : String(error));
    }
  }
  /**
   * What one loaded body answers with, as a factory.
   *
   * A body that handed an `{init}` object over — a `.lynx.bundle`'s
   * `/app-service.js`, or a `RuntimeWrapperWebpackPlugin` banner — is
   * initialized by calling it with this realm's app object, which is
   * lynx-core's own `_$executeInit` (`app.ts`). Anything else *is* the answer:
   * a raw `CommonJS` body's `module.exports`, or a `.json` file's parsed
   * value.
   *
   * `globalThis.globDynamicComponentEntry` is published for the length of the
   * `init` call and restored after it, because a banner reads it there.
   */
  function factoryOf(value: ModuleValue, entry: string): () => ModuleValue {
    const init: unknown = value?.init;
    if (typeof init !== "function") return () => value;
    return () => {
      const previousEntry = globals.globDynamicComponentEntry;
      globals.globDynamicComponentEntry = entry;
      try {
        return init.call(value, {tt: app});
      } finally {
        if (previousEntry === undefined) delete globals.globDynamicComponentEntry;
        else globals.globDynamicComponentEntry = previousEntry;
      }
    };
  }
  /**
   * One body, loaded and run, as the factory of what it answers.
   *
   * The load is the synchronous one a `require` is written over, of the URL
   * its path or section name named. How the source is read is Node's rule, the *response*
   * URL's own extension first and the engine's syntax detection where that
   * says nothing, so three shapes come back:
   *
   * - an **ES module**, which is what a body this container carried is: `ContainerSources` wrote it,
   *   and its default export is the value native's host would have kept as that script's
   *   completion value — the compiler's `{init}` object for a `.lynx.bundle` body,
   *   `module.exports` for a `.web.bundle` one, the parsed value for a `.json` one. A
   *   hand-written module with no `default` export answers its namespace instead.
   * - **JSON**, the value the host parsed.
   * - a **`CommonJS` file**, which is what a path no container carried normally is: compiled in
   *   `module, exports` alone and called with `this` undefined, so none of the Lynx names
   *   `BTS_CHUNK_PREAMBLE` gives a registered body is in scope. It answers `module.exports`.
   *
   * The load parks the job this call runs in. The text never becomes a value
   * here, and a URL is evaluated once per realm however often it is asked for.
   */
  function loadBody(url: string, entry: string): () => ModuleValue {
    const loaded = loadModuleSync(url, "module, exports");
    if (loaded.kind === "json") return factoryOf(loaded.value, entry);
    if (loaded.kind === "module") {
      const namespace = loaded.value as {default?: ModuleValue};
      return factoryOf(
        Object.hasOwn(namespace, "default") ? namespace.default : namespace, entry);
    }
    const module = {exports: {} as ModuleValue};
    (loaded.value as Function).call(undefined, module, module.exports);
    return factoryOf(module.exports, entry);
  }
  /**
   * `lynx.requireModule`: the exports of one bundle path, initialized once per
   * realm.
   *
   * The key of both tables is the bare `path`, the entry no part of it, as in
   * lynx-core (`app.ts` `_$factoryCache`). Nothing is cached until the factory
   * has returned, so a path whose `init` threw is loaded and initialized again
   * by the next call — the load answering from the module this realm already
   * evaluated.
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
    const factory = factories.get(path) ?? loadBody(pathURL(path, entry), entry);
    // Both tables are written after the factory returned, as lynx-core writes
    // `_$factoryCache` only there (`app.ts` `loadScript`): a factory whose
    // `init` threw is not retained either, so the next call runs the whole
    // path again.
    const value = factory();
    factories.set(path, factory);
    cache.set(path, value);
    return value;
  }
  /**
   * `nativeApp.loadScript(sourceURL, entryName)`: the `{init}` object web-core
   * answers with (`createChunkLoading.ts` `createBundleInitReturnObj`), whose
   * `init` answers the module's exports — the chunk's own `init({tt})` where
   * its body handed one over, `module.exports` for a raw body, the parsed
   * value for JSON.
   *
   * Neither of `requireModule`'s two tables is written: lynx-core's
   * `loadScript` feeds neither either, so a `requireModule` of the same path
   * afterwards asks for it again — which is another load, answered from the
   * module this realm already has.
   *
   * The load itself is this call's, not `init`'s, as web-core's is: a raw
   * body has therefore already run by the time `init` is callable. `init`
   * ignores the injection it is handed, this realm having one app object,
   * which is the one every factory here is given.
   */
  function loadScriptInit(sourceURL: string, entry = DEFAULT_ENTRY) {
    const factory = loadBody(pathURL(sourceURL, entry), entry);
    return {init: (_inject?: ModuleValue): ModuleValue => factory()};
  }
  /**
   * `lynx.loadScript(key, {bundleName})`: one named custom section, answered
   * once per entry, from the section URL `sectionURL` writes under the
   * entry's container.
   *
   * A section is loaded like any other body of its container and answers what
   * any other one does — the module's default export, initialized when that
   * carries an `init` — which
   * is web-core's `createBundleInitReturnObj` result rather than native's
   * Script completion value. Where the two disagree this engine takes
   * web-core's, so a `.web.bundle` section is written `module.exports = …`
   * while a `.lynx.bundle` section, whose bodies are expressions, is the
   * expression itself.
   */
  function loadScript(key: string, options: {bundleName?: string}): ModuleValue {
    const entry = options.bundleName ?? DEFAULT_ENTRY;
    const cacheKey = `${entry}:${key}`;
    if (sectionExports.has(cacheKey)) return sectionExports.get(cacheKey);
    const value = loadBody(sectionURL(key, requireContainer(key, entry)), entry)();
    sectionExports.set(cacheKey, value);
    return value;
  }
  /**
   * One entry's container URL, which every body of that entry hangs under.
   *
   * That is all a bundle is here — its bodies are registered resources the
   * boot script never touches, each loaded by the `requireModule`,
   * `loadScript` or `loadScriptInit` that asks for it. No URL is no
   * container: the default entry registered without one loads nothing but an
   * absolute path.
   */
  function registerTemplateUrl(url: string | undefined, entry = DEFAULT_ENTRY) {
    if (url !== undefined) templateUrls.set(entry, url);
  }
  return {define, require, requireModule, loadScript, loadScriptInit,
    registerTemplateUrl};
}
