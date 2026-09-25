# Worker resource loading

Worker ESM loading already supplies the transport needed by raw BTS entries.
The remaining ReactLynx compiled-module layer must follow the framework's
callers and generated wrappers. Bypassing `lynx_core.js` does not require its
`requestScript` or `readScript` interfaces, or a separate protocol returning
source text to JavaScript callbacks.

## Existing ESM transport

`WorkerStart` carries a `HostOutbox` (`WorkerStart.sources`) and the worker's
own cancellation token, which no view token is a parent of. Discovered imports
use `SourceRequest::Module` on the view's existing notice channel.
`LynxView::pump` calls `ResourceFetcher::request_source`; the concrete
`SourceCompletion` answers the requesting worker directly. A `Module` request
arrives absolute: an import is normalized against its importer's response URL,
a worker script is joined to the creating view's entry URL, and a synchronous
load is resolved against the view's `ViewSources::base_url`, which the
worker's `HostOutbox` carries. Transport, UTF-8 validation, and the final
response URL belong to the fetcher. MTS does not route resource replies.

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
| `lynx.requireModule(path, bundleName)` | Generated entry loads an embedded module and synchronously receives its exports. `packages/webpack/template-webpack-plugin/src/LynxEncodePlugin.ts`. **Implemented**: one synchronous load of the URL the path names beside the registered template URL, whether or not the container carried it. |
| `lynx.requireModuleAsync(url, callback)` | Dynamic JS imports and generated JS chunk loading receive `(error, exports)`. `packages/react/runtime/src/core/lynx/dynamic-import.ts` and `packages/webpack/chunk-loading-webpack-plugin/src/runtime/javascript/chunk-loading.js`. |
| `lynx.fetchBundle(url, {})` | Default asynchronous lazy loading calls the returned handler's `.then(callback)` and reads `code` and `url`. `packages/react/runtime/src/core/lynx/lazy-bundle.ts`. **Implemented**: one `SourceRequest::Fetch` — a plain fetch, which the fetcher may make a container of; the handle is native's `{wait, then}` object over one `bobcat:future` `Future`, not a Promise, and a URL the fetcher's `fetch_probe` already holds settles at once. |
| `fetchBundle(...).wait(5)` | Only a lazy import explicitly using `mode: 'sync'` takes this path. Ordinary asynchronous lazy loading does not wait synchronously. Same lazy-bundle source. **Implemented**: a number of *seconds*, over `bobcat:future`'s `Future.wait`, which parks the job the way a `require` does. |
| `lynx.loadScript('background', { bundleName })` | Executes the loaded bundle's BTS section and synchronously returns its result. Same lazy-bundle source. **Implemented** on both threads: one synchronous load of the section URL that `bundleName` names. |
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

The asynchronous APIs above remain unimplemented; the lazy-bundle ones do
not — see "Lazy containers" below.
`callLepusMethod` already supplies the message boundary. They should reuse the
existing resource transport while providing execution results, exports,
caching, and errors at the required API boundary. An `import()` is the loading
half and not the API: a Lynx module factory is initialized through its own
`init({tt})` and cached under the bare path, and a section is answered once per
entry, none of which the module map does by itself.

Node's `require` is the *synchronous* primitive beside it, as `bobcat:module`:
`createRequire(import.meta.url)` resolves through the normalizer imports use,
requests `SourceRequest::Module` on this same channel, and parks the job it
runs in on the answer — the engine thread's
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

`lynx.requireModule(path, entryName?, options?)` is the compiled-bundle layer,
in `bobcat:lynx-modules`, and it is **that same synchronous load**, one
mechanism with MTS's `__LoadLepusChunk`: the realm builds the URL the path
names and loads it, before the call returns. There is no table of bodies and no
boot-time import. Nothing is registered as source text, and nothing hands a
body to JavaScript.

`PageSource` turns every body of a container — its manifest paths and its
string custom sections — into an **ES module** and registers each with the
embedder's resource system, beside the page's own input URL. The BTS boot
script it writes registers that URL and starts the card, and carries nothing of
the container's bodies — no name, no URL, no text:

```js
import {lynx, __BobcatRegisterBundle} from 'bobcat:bts-runtime';
__BobcatRegisterBundle("<template url>");
lynx.requireModule('/app-service.js');
```

That `requireModule` runs inside this module's own evaluation, and the load it
makes is a `require(esm)` of a module nothing has reached yet, so the
still-evaluating refusal above does not apply: a bundle body is never
`import`ed, only `require`d, and is therefore either compiled by the `require`
that asked for it or answered from the evaluation an earlier one already ran.
The body's own `BTS_CHUNK_PREAMBLE` import of `bobcat:bts-runtime` links to
the instance the realm already has, and a chunk a card's `init` reaches for is
another such load. A native container's main-thread sections are not among the
bodies, being Lepus chunks — a chunk is a plain script resource the MTS realm
loads on demand, not a module anything imports.

**What a body becomes.** One physical line of preamble, so the body keeps its
own line numbering. The preamble is `BTS_CHUNK_PREAMBLE`
(`crates/bobcat-core/src/esm.rs`): every name web-core's chunk wrapper would
have had as a parameter (`createChunkLoading.ts`
`createBundleInitReturnObj`) — the ones this realm has a value for imported
from `bobcat:bts-runtime`, the rest `undefined`. Then, by container:

- a `.lynx.bundle`'s body is one expression statement — the Lynx compiler's
  `(function(){…})()`, or a `RuntimeWrapperWebpackPlugin` banner — so it
  becomes `export default <body>`. What native's host would have kept as that
  script's completion value is the module's default export instead. A trailing
  `;` and a `//# sourceMappingURL=` line are both fine after
  `export default <expr>`.
- a `.web.bundle`'s body is a CommonJS file, so it gets a `module` object and
  an `exports` alias of its own and `export default module.exports` after it.
  Its leading `"use strict"` stops being a directive prologue; a module is
  strict anyway.
- a `.json` body is a value rather than a file, and is registered **verbatim**:
  the loader reads a `.json` response as JSON by its own path, so there is
  nothing a module wrapper around it could be compiled as.

**What a load answers.** The path is rooted first — native's own rooting
(`js_app.cc` `App::LoadScript`), so a manifest path (`/app-service.js`) and a
section name (`background`) are each reachable by either spelling, both naming
one URL — and taken as a reference beside the registered template URL, so
`/chunk.js` under `https://cdn.test/app/x.web.bundle` is
`https://cdn.test/app/chunk.js`. Then, by what came back:

- a **module**, which is what a registered body is: its `default` export, or the namespace itself
  when it has no `default` own property, which only a hand-written module has.
- **JSON**: the value the host parsed.
- **CommonJS**, which is what a path no container carried normally is: compiled in `module,
  exports` and nothing else, with `this` undefined — such a file gets none of the Lynx names a
  registered body has — and answering `module.exports`.

That value is then the answer, except that a value carrying an `init` function
is *initialized*: `init.call(value, {tt: app})`, lynx-core's `_$executeInit`
(`app.ts`), with `globalThis.globDynamicComponentEntry` published for the
length of that call because a banner reads it there. That is how a compiled
card starts.

Nothing is cached until the factory has returned, and the key is the bare
path, as in lynx-core: a value whose `init` threw is loaded and initialized
again by the next call — the load answering from the module the realm has
already evaluated, since a URL is one module per realm.

With no template URL registered only an absolute path resolves, and a bundle
path is a `TypeError` carrying the normalizer's message. An entry no
`__BobcatRegisterBundle` named at all is a *lazy container's* `bundleName`,
and its sections are named by the rule below rather than resolved beside
anything.

`nativeApp.loadScript(sourceURL, entryName?)` on `lynx.getNativeApp()` is the
same load, answering the `{init}` object web-core's
`createBundleInitReturnObj` answers with — the load is that call's, not
`init`'s. It writes neither of `requireModule`'s caches, as lynx-core's
`loadScript` writes neither.

`lynx.loadScript(key, {bundleName})` is the named custom sections, the same
load, cached once per entry and key. A section's value is what its own body
evaluated to, which is web-core's `createBundleInitReturnObj` result rather
than native's Script completion value: where the two disagree this project
takes web-core's, so a `.web.bundle` section is written
`module.exports = 21 * 2` while a `.lynx.bundle` section, whose bodies are
expressions, is `21 * 2`.

Choices here that are this engine's, not native's: there is no fetch timeout
(native defaults to 5 s and `requireModule`'s `options` never reaches it
anyway, its `loadScript` binding reading a timeout only from a *number* third
argument), a load the host cannot answer carries this engine's own
`cannot load '<url>'` text, `Card`/`Component` are always in scope for a
registered body where web-core omits the pair for a React card, and **a body
evaluates exactly once per realm**, at the first call that asks for it, where
native re-evaluates per call — see `docs/tracking/deviations.md`.
`lynx.requireModuleAsync`, `nativeApp.loadScriptAsync` and
`nativeApp.readScript` are still absent.

## Lazy containers

`lynx.fetchBundle(url, options?)` is **a plain fetch** — apart from being
waitable, it does what `fetch(image_url)` does — and nothing about a Lynx
container is `bobcat-core`'s business. It exists in both realm kinds, because
either thread's half of a ReactLynx `lazy()` may be the one that asks.

**The request.** `SourceRequest::Fetch { url }` carries the string the card
passed; resolution is the fetcher's, against its own base, as it is for a
stylesheet. A host
answers `LoadedSource::Fetched` — which carries nothing — once the fetch is
over, or fails the request. Core never learns what came back, and there is no
`Bundle` request, no installed set and no record JSON anywhere in it: the one
member is `fetchResource(url)`, which answers the id of the `bobcat:future`
`Future` that fetch settles, or `true` for a URL the fetcher says this view
already has (`crates/bobcat-core/src/fetch.rs`).

**The decode is the fetcher's.** Whether those bytes were a Lynx container
whose sections should be registered is decided in
`bobcat_resources::ContainerInstaller`, whose one implementation is
`bobcat_source::LazyBundleInstaller`. It **sniffs first** — a native or a web
container's magic — and answers `Ok(false)` for anything else, which leaves
the fetch a plain fetch that completed. A host that configures no installer
still fetches; only a container that will not decode fails the request.

**The section URL rule.** One rule, shared by the installer and both realms:
a container at `<url>` answers its section `<name>` at
`<url path>/<encoded name>.js`, keeping the container URL's `?#` suffix, and
its named stylesheet `CSS` at `<url path>/index.css`. That is
`named_chunk_url`/`named_style_url` in `crates/bobcat-source/src/page.rs` and
`packages/bobcat-element/src/section-url.ts` on the realm side, which MTS's
`chunkURL`/`styleSheetURL` and BTS's `bodyUrl` both go through. One leading
`/` is stripped first, so either spelling of a name is one URL. The realms
write that URL as the container was named, rooted or relative, and load it
through `loadModuleSync`, which resolves it against the view's
`ViewSources::base_url` in Rust. The container's fetch — and so the URLs the
installer registers its sections at — and a `__LoadStyleSheet('CSS')` of its
`index.css` are resolved by the fetcher against its own base. The URL a
section is registered under and the URL a realm loads it by are equal only
because the embedder gives its fetcher the view's base: every embedder
in this workspace does, the CLI and the server through `PageSource`'s input
URL and the browser through `load_sources`. A
`main-thread` section becomes an MTS module — `MTS_CHUNK_PREAMBLE`, the entry's
own binding list, then `export default <the body>` — and everything else the
BTS module `bts_module_source` already wrote. The container's *own* StyleInfo
is not registered: native applies a lazy bundle's CSS only through
`__LoadStyleSheet('CSS')`.

**What has been fetched is the fetcher's, and it answers through a probe.**
Core remembers nothing. `ResourceFetcher::fetch_probe()` hands the realms one
`Send + Sync` function — the only part of a host's resource system that leaves
the embedder's thread — and `fetchResource` asks it before requesting
anything: a URL this view already fetched answers `true` **in the same call**,
with no request at all. That is this engine's
`TemplateAssembler::FindTemplateBundle`, and it is load-bearing rather than an
optimisation: a repeat `fetchBundle`'s handle is settled from the start, so
MTS runs its `.then` inline, which is what `rLynxPrepareLazyBundleMTS` needs
(`crates/bobcat-source/tests/lazy_bundle.rs` is the end-to-end proof, over the
real compiled `react-lazy` fixture). The reference fetcher's set is written
only when a `Fetch` load **completed successfully**, after the installer ran;
a failed fetch and a failed install are not remembered, as native remembers no
failure, and a host that offers no probe simply never answers `true`.

**The handle** is native's host object and not a Promise: exactly `wait` and
`then`, `.then` answers `undefined`, and there is no chaining. `options` is
accepted and ignored. A fetch that had to be made is one **`bobcat:future`
`Future`** — the host member answers its id — and both members are that
Future's, so neither the wait nor the asynchronous settle is machinery of this
feature's own; a fetch the probe answered `true` for needs no Future at all
and its handle is settled from the start. The `{url, code, error_msg}` record
is built in `bundle-fetch.ts`, out of that outcome and the URL the caller
passed.

- `wait(seconds)` is `Future.wait(seconds * 1000)`: it parks the *job* it runs in — the park a `require` makes, so this engine thread's tasks go on running and no other job does — until the fetch settles, the realm ends, or the deadline passes. The `TimeoutError` that last one throws becomes `{url, code: -2, error_msg: "ResponsePromise wait timeout after <t> seconds for url: <url>"}`, and **cancels nothing**: the Future goes back into the host's table, so a later `wait` or `then` still sees the result. `Infinity` seconds is `Infinity` milliseconds, which is the Future's own "no deadline at all".
- `then(callback)` runs the callback when the fetch settles. **Settled means this realm holds the outcome**, whichever way it arrived — the probe's `true` at the `fetchBundle`, a `wait` that returned, or the Future's Promise — and not a delivery having happened: `h.wait(5); h.then(cb)` runs `cb` at the `then`, because the `wait` is what brought the outcome in. That is native's `LynxActor::Act`, which acts on the value being there, and for that case the timing still differs by thread as native's does: **MTS runs it inline** and **BTS posts it** (`bts_runtime_mediator`). A callback registered while the fetch was still outstanding runs as a reaction of the Future's Promise instead, each in its own try/catch: one that throws is reported and the next still runs, and none of them can become an unhandled rejection.
- The first such callback is what converts the Future, once. That conversion is one-way, so a `wait` **after** a `then` on the same handle throws a `TypeError` — `bobcat:future`'s structural refusal, since the delivery is a job and a job cannot run inside another job's wait. Native's `shared_future` allows the pair; see `docs/tracking/deviations.md`.

A settled record is `{url, code, error_msg}`: `code` `0` fetched, `-1` the
fetch failed (carrying the host's reason), `-2` a `wait` timeout. `url` is
**the string the caller passed**, echoed, because ReactLynx uses it as the
`bundleName` of every later `loadScript` and `__LoadStyleSheet` and keys its
own cache by it.

Delivery is `bobcat:future`'s: the `.then` hands the load to the realm's owner
through `settleFuture`, the owner's epilogue — `Page`'s on `bobcat-main`,
`Worker`'s on `bobcat-workers` — spawns one task that awaits it, and that task
*enters the realm* to resolve the Promise. The callbacks are that Promise's
reactions, so a callback that builds elements or adopts a stylesheet runs
inside an entry and gets the epilogue it owes. Native's BTS posts a task and
its MTS posts to the Lepus thread; both are asynchronous, which is the same
shape.

There is no `ScriptLoad` queue, `SourceRequest::Script` variant, synchronous
`readScript` binding, or JS source-callback registry, and no API that hands a
source's text to JavaScript: the host compiles or parses it inside the engine
and answers with a function or a parsed value. Tests should exercise the existing
ESM/require/readiness/cancellation path and, when implemented, the actual
ReactLynx caller contracts rather than reintroducing a private text-read API.
