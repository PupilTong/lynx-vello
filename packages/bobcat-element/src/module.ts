import { loadModuleSync, resolveModuleUrl } from "bobcat-internal:host";

// Node's `createRequire` and the `require` it answers, preloaded as the
// `bobcat:module` ESM. Every realm this engine builds has it: the views'
// main-thread realms on one runtime, and each worker realm on the group's
// worker runtime. It is an explicit import — no entry preamble carries it.
//
// # What lives here and what lives in the host
//
// The algorithm: the cache, the `module` object, the wrapper call, cycles,
// eviction, `require.resolve`. All of it is ordinary JavaScript, because all
// of it is about values in this realm.
//
// The host has the two steps that are not. `resolveModuleUrl` is the
// normalizer an `import` resolves through, so a `require` and an `import` name
// the same module by the same URL. `loadModuleSync` asks for one URL and
// answers with the source already compiled: the wrapper function of a
// CommonJS file, or the parsed value of a JSON one. Source text never becomes
// a value here, the compile is named by the URL the load answered from, and
// the load parks the job this call runs in — no promise job of this realm's
// runs while it waits.
//
// # Deviations from Node
//
// There is no file system and no resolution algorithm of Node's: a specifier
// is a URL or a relative reference, a bare name is refused, and there are no
// `node_modules`, no `package.json` `exports`, no extension search and no
// directory index. A specifier that cannot be resolved is a `TypeError`
// carrying the normalizer's message, where Node reports an `Error` with
// `code: "MODULE_NOT_FOUND"`. Which of CommonJS and JSON a source is read as
// is the response URL's own path extension. `require.main`,
// `require.extensions` and `module.parent` are absent, and a `require` of ESM
// text is a `SyntaxError` rather than Node's own diagnostic.

/** The parameter list every CommonJS wrapper is compiled with. */
const WRAPPER_PARAMETERS = "exports, require, module, __filename, __dirname";

/**
 * A cache entry while `require` is still writing it. `RequiredModule` is the
 * read view the same object is handed out as.
 */
interface ModuleEntry extends RequiredModule {
  exports: unknown;
  loaded: boolean;
}

/** What one CommonJS wrapper is called as. */
type Wrapper = (
  exports: unknown,
  require: Require,
  module: RequiredModule,
  filename: string,
  dirname: string,
) => unknown;

/**
 * The realm's one CommonJS cache, shared by every `require` in it.
 *
 * Null-prototype, so a URL that happens to name an `Object.prototype` member
 * is a miss rather than an inherited value, and so a card that deletes a key
 * makes the next `require` load that URL again.
 */
const cache: Record<string, ModuleEntry | undefined> = Object.create(null);

/**
 * A `require` that resolves specifiers against `base`.
 *
 * `base` is checked here, where Node checks it too: a base that is not a
 * string is a `TypeError` at the call rather than at whichever resolution
 * happens to reach the host first.
 */
export function createRequire(base: string): Require {
  if (typeof base !== "string") {
    throw new TypeError("createRequire expects a URL string");
  }
  function require(specifier: string): unknown {
    return load(resolveUrl(base, specifier));
  }
  return Object.assign(require, {
    resolve(specifier: string): string {
      return resolveUrl(base, specifier);
    },
    cache,
  });
}

/**
 * The URL `specifier` names against `base`, with a refusal as a `TypeError`.
 *
 * Every host member reports a failure as an `InternalError`, so the class a
 * specifier this engine cannot resolve reaches author code as is this realm's
 * to choose, and the normalizer's own message is what it carries.
 */
function resolveUrl(base: string, specifier: string): string {
  try {
    return resolveModuleUrl(base, specifier);
  } catch (error) {
    throw new TypeError(error instanceof Error ? error.message : String(error));
  }
}

/** One already-resolved URL, from the cache or from a load. */
function load(url: string): unknown {
  const cached = cache[url];
  if (cached !== undefined) {
    return cached.exports;
  }
  // The URL asked for is the cache key, `id` and `filename`. The URL the load
  // answered from is what a nested `require` resolves against, what
  // `__filename` is, and what `__dirname` is one resolution away from: a
  // redirect moves the file, not the name it was required by.
  const loaded = loadModuleSync(url, WRAPPER_PARAMETERS);
  if (loaded.kind === "json") {
    // Parsed before it was answered, and there is no body to run: the entry
    // is complete the moment it is made.
    const parsed: ModuleEntry = {
      id: url,
      filename: url,
      exports: loaded.value,
      loaded: true,
    };
    cache[url] = parsed;
    return parsed.exports;
  }
  const module: ModuleEntry = {
    id: url,
    filename: url,
    exports: {},
    loaded: false,
  };
  // In the cache before the body runs, which is what gives a cycle the
  // exports the other module has so far. A body that throws takes it back
  // out, so the next `require` loads the file again.
  cache[url] = module;
  try {
    (loaded.value as Wrapper).call(
      module.exports,
      module.exports,
      createRequire(loaded.url),
      module,
      loaded.url,
      resolveUrl(loaded.url, "./"),
    );
  } catch (error) {
    delete cache[url];
    throw error;
  }
  module.loaded = true;
  // Re-read: the body may have assigned over `module.exports`.
  return module.exports;
}
