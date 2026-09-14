// Compiler factory ABI for sources already registered by PageSource.
// External fetching and lazy containers are handled separately from this table.
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

// Compiler modules choose their own export shape and replace module.exports.
// This boundary deliberately carries those JS values without a runtime schema.
type ModuleValue = any;
interface Definition { factory: Function; hasRun: boolean; exports?: ModuleValue }
interface Source { source: string; wrapped: boolean }

export function createLynxModules(app: ModuleValue, lynx: ModuleValue, console: ModuleValue) {
  const globals = globalThis as unknown as { bundleSupportLoadScript: boolean; globDynamicComponentEntry?: string };
  globals.bundleSupportLoadScript = true;
  const definitions = new Map<string, Map<string, Definition>>();
  const sources = new Map<string, Map<string, Source>>();
  const sections = new Map<string, Record<string, string>>();
  const cache = new Map<string, ModuleValue>();
  const factories = new Map<string, () => ModuleValue>();
  const sectionExports = new Map<string, ModuleValue>();

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
  function requireModule(path: string, entry = DEFAULT_ENTRY): ModuleValue {
    const cached = cache.get(path);
    if (cached) return cached;
    let factory = factories.get(path);
    if (!factory) {
      const resource = findSource(path, entry);
      if (!resource) throw new Error(`bundle source ${path} in ${entry} is not registered`);
      if (path.split("?")[0]?.endsWith(".json")) {
        const value: unknown = JSON.parse(resource.source);
        factory = () => value;
      } else {
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
            factory = () => bundle.init({tt: app});
          } else {
            const value = module.exports || bundle;
            factory = () => value;
          }
        } finally {
          if (previousEntry === undefined) delete globals.globDynamicComponentEntry;
          else globals.globDynamicComponentEntry = previousEntry;
        }
      }
      factories.set(path, factory);
    }
    const value = factory();
    cache.set(path, value);
    return value;
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
  return {define, require, requireModule, loadScript, register, registerSections};
}
