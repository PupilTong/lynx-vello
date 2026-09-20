// Behavior tests for `bobcat:module` over a recording host mock.
//
// These pin the semantics that live in module.ts: the cache and its key, the
// `module` object, the wrapper call, cycles, eviction, `require.resolve` and
// the response-versus-request URL split. The two members underneath are the
// host's, and are stood in for here — the mock resolves with Node's own `URL`,
// refusing a bare name as the engine's normalizer does, and compiles with
// `new Function`, neither of which the engine does: there, resolution is the
// normalizer an `import` uses and compilation happens inside QuickJS with the
// source text never becoming a value. The real boundary runs
// in crates/bobcat-core/src/main/runtime/tests.rs,
// crates/bobcat-core/src/background/tests.rs and
// crates/bobcat-core/tests/require.rs.

import { beforeEach, describe, expect, it, rstest } from "@rstest/core";
import { createRequire } from "../src/module.ts";

// Only the functions this returns read the tables below, and they run when
// `module.ts` calls them: the factory is hoisted above this file's bindings.
rstest.mockRequire("bobcat-internal:host", () => ({
  resolveModuleUrl: (base: string, specifier: string): string =>
    resolveModuleUrl(base, specifier),
  loadModuleSync: (url: string, parameters: string): LoadedModuleSource =>
    loadModuleSync(url, parameters),
}));

/** One file the mock host serves. */
interface HostFile {
  text: string;
  /** Where it answers from, when that differs from where it was asked for. */
  response?: string;
  kind?: "commonjs" | "json";
}

const files: Map<string, HostFile> = new Map();
/** Every URL a load was asked for, in order. */
const loads: string[] = [];
/** The `parameters` argument of every load, in order. */
const parameterLists: string[] = [];

/**
 * As strict as the engine's normalizer, and as plain in how it says so: every
 * host member reaches JavaScript as an `InternalError`, so a `TypeError` in
 * the assertions below can only have come from module.ts.
 */
function resolveModuleUrl(base: string, specifier: string): string {
  if (typeof base !== "string") {
    throw new Error("resolveModuleUrl expects a string for argument 0");
  }
  if (!/^\.{0,2}\//.test(specifier) && parsed(specifier) === undefined) {
    throw new Error(`cannot resolve module '${specifier}' from '${base}'`);
  }
  return new URL(specifier, base).href;
}

/** `specifier` as the URL it is on its own, or nothing if it is a bare name. */
function parsed(specifier: string): URL | undefined {
  try {
    return new URL(specifier);
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
  const response = file.response ?? url;
  if (file.kind === "json") {
    return { url: response, kind: "json", value: JSON.parse(file.text) };
  }
  return {
    url: response,
    kind: "commonjs",
    value: new Function(...parameters.split(", "), file.text),
  };
}

function file(url: string, text: string, response?: string): void {
  files.set(url, response === undefined ? { text } : { text, response });
}

function json(url: string, text: string): void {
  files.set(url, { text, kind: "json" });
}

/** A counter a file's own body keeps, which only `globalThis` can carry. */
function counter(name: string): number {
  return (Reflect.get(globalThis, name) as number | undefined) ?? 0;
}

describe("Node's require over one synchronous host load", () => {
  beforeEach(() => {
    files.clear();
    loads.length = 0;
    parameterLists.length = 0;
    // The cache is the realm's, so it outlives one test.
    const { cache } = createRequire("app:///entry.js");
    for (const key of Object.keys(cache)) {
      delete cache[key];
    }
    Reflect.set(globalThis, "__moduleRuns", 0);
    Reflect.set(globalThis, "__moduleAttempts", 0);
  });

  it("answers the exports and runs the body with `this` bound to them", () => {
    file(
      "app:///a.cjs",
      "if (this !== exports) throw Error('this is not exports');\n" +
        "if (typeof require !== 'function') throw Error('no require');\n" +
        "exports.answer = 42;",
    );
    file("app:///reassign.cjs", "module.exports = function () { return 7; };");
    const require = createRequire("app:///entry.js");

    expect(require("./a.cjs")).toEqual({ answer: 42 });
    expect((require("./reassign.cjs") as () => number)()).toBe(7);
    expect(loads).toEqual(["app:///a.cjs", "app:///reassign.cjs"]);
  });

  it("compiles every file with the one wrapper parameter list", () => {
    file("app:///a.cjs", "exports.answer = 42;");
    createRequire("app:///entry.js")("./a.cjs");

    expect(parameterLists).toEqual([
      "exports, require, module, __filename, __dirname",
    ]);
  });

  it("resolves a nested require against the response URL", () => {
    file(
      "app:///alias",
      "exports.value = require('./sibling.cjs').value;",
      "app:///deep/nested.cjs",
    );
    file("app:///deep/sibling.cjs", "exports.value = 42;");

    expect(createRequire("app:///entry.js")("./alias")).toEqual({ value: 42 });
    expect(loads).toEqual(["app:///alias", "app:///deep/sibling.cjs"]);
  });

  it("names the response URL as `__filename` and its directory as `__dirname`", () => {
    file(
      "app:///alias",
      "exports.filename = __filename; exports.dirname = __dirname;",
      "https://cdn.test/deep/nested.cjs",
    );

    expect(createRequire("app:///entry.js")("./alias")).toEqual({
      filename: "https://cdn.test/deep/nested.cjs",
      dirname: "https://cdn.test/deep/",
    });
  });

  it("names the request URL as the module's `id`, `filename` and cache key", () => {
    file(
      "app:///alias",
      "exports.id = module.id; exports.filename = module.filename;",
      "https://cdn.test/nested.cjs",
    );
    const require = createRequire("app:///entry.js");

    expect(require("./alias")).toEqual({
      id: "app:///alias",
      filename: "app:///alias",
    });
    expect(require.cache["app:///alias"]?.loaded).toBe(true);
    expect(require.cache["https://cdn.test/nested.cjs"]).toBeUndefined();
  });

  it("loads and evaluates a URL once however often it is required", () => {
    file(
      "app:///counter.cjs",
      "globalThis.__moduleRuns += 1; exports.runs = globalThis.__moduleRuns;",
    );
    const first = createRequire("app:///entry.js")("./counter.cjs");
    const again = createRequire("app:///other/entry.js")("app:///counter.cjs");

    expect(again).toBe(first);
    expect(counter("__moduleRuns")).toBe(1);
    expect(loads).toEqual(["app:///counter.cjs"]);
  });

  it("gives a cycle the exports the other module has so far", () => {
    file(
      "app:///a.cjs",
      "exports.name = 'a';\n" +
        "exports.fromB = require('./b.cjs').name;\n" +
        "exports.late = 'late';",
    );
    file(
      "app:///b.cjs",
      "exports.name = 'b';\n" +
        "const a = require('./a.cjs');\n" +
        "exports.sawName = a.name;\n" +
        "exports.sawLate = a.late;",
    );
    const require = createRequire("app:///entry.js");

    expect(require("./a.cjs")).toEqual({
      name: "a",
      fromB: "b",
      late: "late",
    });
    expect(require("./b.cjs")).toEqual({
      name: "b",
      sawName: "a",
      sawLate: undefined,
    });
  });

  it("evicts a body that threw and runs it again on the next require", () => {
    file(
      "app:///bad.cjs",
      "globalThis.__moduleAttempts += 1; throw Error('boom');",
    );
    const require = createRequire("app:///entry.js");

    expect(() => require("./bad.cjs")).toThrow("boom");
    expect(require.cache["app:///bad.cjs"]).toBeUndefined();
    expect(() => require("./bad.cjs")).toThrow("boom");
    expect(counter("__moduleAttempts")).toBe(2);
    expect(loads).toEqual(["app:///bad.cjs", "app:///bad.cjs"]);
  });

  it("throws a failing load through and caches nothing", () => {
    const require = createRequire("app:///entry.js");

    expect(() => require("./missing.cjs")).toThrow(
      "cannot load 'app:///missing.cjs'",
    );
    expect(require.cache["app:///missing.cjs"]).toBeUndefined();
  });

  it("answers a JSON file with the parsed value, loaded", () => {
    json("app:///config.json", '{"answer": 42, "list": [1, 2]}');
    const require = createRequire("app:///entry.js");

    const config = require("./config.json");
    expect(config).toEqual({ answer: 42, list: [1, 2] });
    expect(require.cache["app:///config.json"]?.exports).toBe(config);
    expect(require.cache["app:///config.json"]?.loaded).toBe(true);
  });

  it("resolves without loading, into a cache with no prototype", () => {
    const require = createRequire("app:///dir/entry.js");

    expect(require.resolve("./a.cjs")).toBe("app:///dir/a.cjs");
    expect(loads).toEqual([]);
    expect(Object.getPrototypeOf(require.cache)).toBeNull();
  });

  it("loads again once its cache entry is deleted", () => {
    file("app:///counter.cjs", "globalThis.__moduleRuns += 1;");
    const require = createRequire("app:///entry.js");

    require("./counter.cjs");
    delete require.cache["app:///counter.cjs"];
    require("./counter.cjs");

    expect(counter("__moduleRuns")).toBe(2);
    expect(loads).toEqual(["app:///counter.cjs", "app:///counter.cjs"]);
  });

  it("shares one cache between every require of the realm", () => {
    file("app:///a.cjs", "exports.answer = 42;");
    const first = createRequire("app:///entry.js");
    const second = createRequire("app:///other/entry.js");

    expect(second.cache).toBe(first.cache);
    first("./a.cjs");
    expect(second.cache["app:///a.cjs"]?.exports).toEqual({ answer: 42 });
  });

  it("refuses a base that is not a string when it is created", () => {
    expect(() => createRequire(42 as unknown as string)).toThrow(TypeError);
    expect(loads).toEqual([]);
  });

  it("reports a specifier it cannot resolve as a TypeError carrying the host's message", () => {
    const require = createRequire("app:///entry.js");

    expect(() => require("lodash")).toThrow(TypeError);
    expect(() => require("lodash")).toThrow("cannot resolve module 'lodash'");
    expect(() => require.resolve("lodash")).toThrow(TypeError);
    expect(() => require.resolve("lodash")).toThrow(
      "cannot resolve module 'lodash'",
    );
    expect(loads).toEqual([]);
  });
});
