# Worker resource loading

Worker ESM loading already supplies the transport needed by raw BTS entries.
The remaining ReactLynx compiled-module layer must follow the framework's
callers and generated wrappers. Bypassing `lynx_core.js` does not require its
`requestScript` or `readScript` interfaces, or a separate protocol returning
source text to JavaScript callbacks.

## Existing ESM transport

`WorkerStart` carries a `SourceRequester` and a child of the view's cancellation
token. Discovered imports use `SourceRequest::Module` on the view's existing
notice channel. `LynxView::pump` calls `ResourceFetcher::request_source`; the
concrete `SourceCompletion` answers the requesting worker directly. Resolution,
transport, UTF-8 validation, and the final response URL belong to the fetcher.
MTS does not route resource replies.

The Worker uses the same asynchronous QuickJS ESM loader as main. Imports share
one evaluation and namespace per normalized URL in each realm. Response URLs
provide the base for dependencies. Imports and timers continue during entry
top-level await; posted messages wait for entry settlement. Worker termination
or view release cancels outstanding completions and discards late results.

Raw XML background entries use this path and import their runtime bindings from
`bobcat:bts-runtime`. The built-in bootstrap installs a JS initializer and
returns; the first Worker message supplies inputs before the application entry
imports. Later messages wait on that import Promise and are delivered in order
once it settles,
success or failure. An entry that throws is reported
through `reportError` as a nonfatal `WorkerFailed` and leaves BTS running. The
view's readiness is MTS boot finishing and does not involve BTS at all, so an
entry whose top-level await never settles delays messages to BTS but not
readiness.

## ReactLynx compiled-module contract

This inventory was checked against `@lynx-js/react` 0.123.0, Rspeedy 0.16.0,
and the source fixture configuration: `engineVersion: '4.1.0'` with default
ReactLynx options. Rebuilding the six native production fixtures produced
12 BTS manifest sections across 11 containers. None contains `requestScript`,
`readScript`, or the legacy `QueryComponent` path. These are build outputs for
inspection, not files to commit. Source references below name files in the
read-only `lynx-stack` checkout at `f47d3e6a56200bf07d58fc1656712878ea851a3d`.

| Required API | Actual caller and observable result |
| --- | --- |
| `lynx.requireModule(path, bundleName)` | Generated entry loads an embedded module and synchronously receives its exports. `packages/webpack/template-webpack-plugin/src/LynxEncodePlugin.ts`. **Implemented**, for a registered manifest path and for one no manifest carries. |
| `lynx.requireModuleAsync(url, callback)` | Dynamic JS imports and generated JS chunk loading receive `(error, exports)`. `packages/react/runtime/src/core/lynx/dynamic-import.ts` and `packages/webpack/chunk-loading-webpack-plugin/src/runtime/javascript/chunk-loading.js`. |
| `lynx.fetchBundle(url, {})` | Default asynchronous lazy loading calls the returned handler's `.then(callback)` and reads `code` and `url`. `packages/react/runtime/src/core/lynx/lazy-bundle.ts`. |
| `fetchBundle(...).wait(5)` | Only a lazy import explicitly using `mode: 'sync'` takes this path. Ordinary asynchronous lazy loading does not wait synchronously. Same lazy-bundle source. |
| `lynx.loadScript('background', { bundleName })` | Executes the loaded bundle's BTS section and synchronously returns its result. Same lazy-bundle source. |
| `lynx.getNativeApp().callLepusMethod(...)` | Requests `rLynxPrepareLazyBundleMTS`; asynchronous lazy loading resolves after the callback confirms MTS preparation. Same lazy-bundle source. |
| `tt.define(...)` and `tt.require(...)` | The generated wrapper receives `tt` through `init({tt})`, registers a module factory, then obtains its exports. `packages/webpack/runtime-wrapper-webpack-plugin/src/RuntimeWrapperWebpackPlugin.ts`. |

`lynx.loadLazyBundle` is installed by ReactLynx itself. It composes the host APIs
above; the host should not duplicate it. The compiler selects `FetchBundle`
for engine versions at least 3.9 by default
(`packages/rspeedy/plugin-react/src/resolveLazyBundleFetcher.ts`). Supporting
the current engine configuration does not require the legacy `QueryComponent`
and `getDynamicComponentExports` fallback.

Development HMR also obtains its JSON manifest through `requireModuleAsync`
and consumes the parsed result
(`packages/webpack/chunk-loading-webpack-plugin/src/runtime/javascript/hmr-load-manifest.js`).
That caller does not require a separate raw JSON text API.

## Implementation boundary

The asynchronous and lazy-bundle execution APIs above remain unimplemented;
`callLepusMethod` already supplies the message boundary. They should reuse the
existing resource transport while providing execution results, exports,
caching, and errors at the required API boundary. Lynx module factories and
section evaluation have different semantics from ESM; reusing transport does
not make an ordinary `import()` a complete implementation of `requireModule` or
`loadScript`.

The *synchronous* primitive those callers need exists as `bobcat:module`:
`createRequire(import.meta.url)` answers Node's `require`, which resolves
through the normalizer imports use, requests `SourceRequest::Module` on this
same channel, and parks the job it runs in on the answer — the engine thread's
tasks keep running, and no other job does. It keeps one cache per realm, and
reads a source as CommonJS, JSON or an ES module, by Node 24's rules for which:

- the *response* URL's path extension decides, as `crates/bobcat-core/src/require.rs` `kind_of`:
  `.json` is JSON, `.mjs` is a module, `.cjs` is CommonJS. There is no `package.json` `"type"` to
  consult, so anything else — a plain `.js` — is decided by QuickJS's own syntax detection over the
  text, in the bridge, where the text is.
- an ES module is **linked inline**: every `import` in it, and in what it imports, is loaded through
  the same host member during its compile, recursively, each parking its own job, before any body
  runs. An `import` of a built-in (`bobcat:*`) links to that native module instead of being fetched.
- evaluation is synchronous and runs no promise jobs. A graph that awaits at its top level is
  therefore refused — `cannot require '<url>': it uses top-level await …`, Node's
  `ERR_REQUIRE_ASYNC_MODULE` — rather than waited for, and the module is left suspended for the
  realm's own jobs to settle.
- what `require` answers is the namespace object, or, when the namespace has an export literally
  named `module.exports`, that export's value.
- one URL is one module: a URL an `import` already brought into the realm is answered from that
  instance rather than loaded and evaluated again.

A `require` of a module belonging to a graph that is **still evaluating** is
refused for the same reason a cycle is (Node's `ERR_REQUIRE_CYCLE_MODULE`):
re-entering a body that is part-way through would corrupt the evaluation
running it. QuickJS keeps `JSModuleDef`'s status private, so the realm refuses
the whole graph rather than only the cycle — a module the realm reached by an
`import`, asked for by a `require` from inside another module's body, is
refused even when its own body has already finished. A `require` reached from
anywhere no module body is running — a host call, a listener, a timer, a plain
script — answers from the instance as usual.

`lynx.requireModule(path, entryName?, options?)` is the compiled-bundle layer
over the same primitive, in `bobcat:lynx-modules`. A registered manifest path is
evaluated from the source the boot script carried, as before. A path no
manifest carries is *loaded*: the path is rooted the way native roots it
(`js_app.cc` `App::LoadScript`), taken as a reference beside the template URL
the entry's `__BobcatRegisterBundle` was given — the page's own input URL, so
`/chunk.js` under `https://cdn.test/app/x.web.bundle` is
`https://cdn.test/app/chunk.js` — and handed to `loadModuleSync` in web-core's
own wrapper parameter list (`createChunkLoading.ts`
`createBundleInitReturnObj`). A Lynx-target chunk answers through
`globalThis.__bundle__holder`, which is where the
`RuntimeWrapperWebpackPlugin` banner stores its `{init}` while
`bundleSupportLoadScript` is set; a raw CommonJS body answers through
`module.exports`; a `.json` response is the value the host parsed. Nothing is
cached until the load, the compile and the body have all returned, and the key
is the bare path, as in lynx-core. With no template URL registered only an
absolute path resolves, and a bundle path is a `TypeError` carrying the
normalizer's message.

`nativeApp.loadScript(sourceURL, entryName?)` on `lynx.getNativeApp()` is the
same code path, answering the `{init}` object web-core's
`createBundleInitReturnObj` answers with. It consults the registered sources
first and writes neither of `requireModule`'s caches, as lynx-core's
`loadScript` writes neither, so a `requireModule` of that path afterwards loads
it again.

Three choices there are this engine's, not native's: there is no fetch timeout
(native defaults to 5 s and `requireModule`'s `options` never reaches it
anyway, its `loadScript` binding reading a timeout only from a *number* third
argument), a load the host cannot answer carries this engine's own
`cannot load '<url>'` text, and `Card`/`Component` are always among the
wrapper's parameters where web-core omits the pair for a React card.
`lynx.requireModuleAsync`, `nativeApp.loadScriptAsync`, `nativeApp.readScript`,
`lynx.fetchBundle` and the lazy-bundle `lynx.loadScript` for an unregistered
bundle are still absent.

There is no `ScriptLoad` queue, `SourceRequest::Script` variant, synchronous
`readScript` binding, or JS source-callback registry, and no API that hands a
source's text to JavaScript: the host compiles or parses it inside the engine
and answers with a function or a parsed value. Tests should exercise the existing
ESM/require/readiness/cancellation path and, when implemented, the actual
ReactLynx caller contracts rather than reintroducing a private text-read API.
