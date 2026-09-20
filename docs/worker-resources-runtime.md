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
| `lynx.requireModule(path, bundleName)` | Generated entry loads an embedded module and synchronously receives its exports. `packages/webpack/template-webpack-plugin/src/LynxEncodePlugin.ts`. |
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

The compiled-module and lazy-bundle execution APIs above remain unimplemented;
`callLepusMethod` already supplies the message boundary. They should reuse the
existing resource transport while providing execution results, exports,
caching, and errors at the required API boundary. Lynx module factories and
section evaluation have different semantics from ESM; reusing transport does
not make an ordinary `import()` a complete implementation of `requireModule` or
`loadScript`.

The *synchronous* primitive those callers need does exist now, as
`bobcat:module`: `createRequire(import.meta.url)` answers Node's `require`,
which resolves through the normalizer imports use, requests
`SourceRequest::Module` on this same channel, and parks the job it runs in on
the answer — the engine thread's tasks keep running, and no other job does. It
reads a source as CommonJS or JSON and keeps a CommonJS cache per realm.
`lynx.requireModule`, `lynx.loadScript`, the Lynx wrapper's parameter list and
lynx-core's own module caches are still not implemented and are a layer over
it, not the same thing: a bundle section is not a file at a URL. The wrapper's
parameter list is the one thing of theirs that is already reachable: the host
member underneath, `loadModuleSync(url, parameters)`, compiles a CommonJS
source inside the wrapper parameter list it is handed, and `bobcat:module`
passes Node's five.

There is no `ScriptLoad` queue, `SourceRequest::Script` variant, synchronous
`readScript` binding, or JS source-callback registry, and no API that hands a
source's text to JavaScript: the host compiles or parses it inside the engine
and answers with a function or a parsed value. Tests should exercise the existing
ESM/require/readiness/cancellation path and, when implemented, the actual
ReactLynx caller contracts rather than reintroducing a private text-read API.
