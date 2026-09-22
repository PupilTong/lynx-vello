# Boot microtasks and named Lepus chunks

This layer adds on-demand execution of local named Lepus chunks and defers
boot's final flush through ordinary JavaScript Promise scheduling. Runtime/PAPI
identifiers remain imports in the selected entry module. The existing QuickJS
bridge and outer checkpoint supply all required script execution capabilities.

## Boot calls and errors

Boot calls `processData`, then `renderPage` or the fallback `__RenderPage` engine
event synchronously. A processor's result goes directly to rendering, preserving
object and Promise identity without awaiting it. No jobs run between these
calls or between engine listeners.

Each hook has a JavaScript error boundary. A throwing processor reports and
supplies undefined to rendering; a throwing render hook reports and still
reaches the final flush. Boot dispatches through
`lynx.getEngine().dispatchEvent({ type: '__RenderPage', data })`.
The private engine context implements listener error reporting inside that
standard method and continues the walk. Function listeners receive the engine
as their receiver; object listeners retain their own receiver.
The base `EventTarget` retains its existing semantics.

Boot then runs:

```js
await Promise.resolve().then(() => __FlushElementTree());
```

The flush follows jobs already queued by the hooks. A job may enqueue another
job behind the flush: `render -> job 1 -> flush -> job 2` is expected. The await
keeps the flush in the boot completion/failure path; it does not await hook
results or drain jobs recursively inside a host callback. This being boot's
last act is also where a view waits for a painter to bind it: before the first
binding the flush holds its frame and parks the job it runs in, and a release
while it is parked throws there, which fails the boot. The existing outer
checkpoint continues to report unhandled rejections and enforce its deadline,
and runs the runtime's queued jobs until the queue is empty, as a browser's
microtask checkpoint does: no job budget bounds it, and there is no incomplete
checkpoint for the next entry into the realm to resume instead of running its
own operation. Entry-module and flush failures still fail boot. Later dirty
mutations retain the existing page epilogue's commit behavior.

Named cross-thread calls continue to await through Worker `postMessage` RPC.
Neither boot scheduling nor local chunk loading changes that boundary.

## Named Lepus chunks

`PageSource` registers every non-entry Lepus chunk with the embedder's
resource system, **verbatim**, at a resource URL of its own —
`<entry URL path>/<name>.js`, the chunk name percent-encoded the way a named
stylesheet section's is (`named_chunk_url`), any `?`/`#` suffix of the entry
URL kept. Nothing is prepended to it and nothing imports it. The root MTS
script is the container's own text: no import prefix, no registration call, no
table.

`__LoadLepusChunk(path, options)` does the rest, in JavaScript
(`main-thread-runtime.ts`):

- a different component entry: `false`, as in native, without asking for
  anything;
- otherwise the chunk's URL is built here — `chunkURL`, the same string
  `named_chunk_url` writes — and loaded through `loadModuleSync`, the **same
  synchronous host loader a `require` uses**;
- a load the host cannot answer is a chunk this page does not carry: `false`;
- a load that answers a `SyntaxError` is a chunk that exists but does not
  compile: reported through `_ReportError`, and `true`;
- otherwise the body runs, an exception in it is reported and nothing more,
  and the answer is `true`.

The load parks the job the call runs in until the host answers: `bobcat-main`'s
tasks keep running, and no other job does — not this realm's promise jobs, and
not a sibling realm's. **A chunk is evaluated again on every call**, as
native's `TemplateEntry` does (`core/renderer/template_entry.cc`,
`core/runtime/lepus/bindings/renderer_functions.cc`): there is no chunk cache,
and the ESM module map never sees a chunk, because a chunk is not a module.

A chunk does not share the entry module's lexical scope. The host compiles it
as a **function body**, whose parameters are the bindings the entry preamble
gives the entry — every `bobcat:element` export, and the `bobcat:runtime`
names `__Card__`, `lynx`, `console`, `SystemInfo`, `__globalProps`,
`NativeModules`, `_AddEventListener`, `_ReportError`, `_SetSourceMapRelease`,
`__OnLifecycleEvent`, `__LoadLepusChunk`, `__LoadStyleSheet` and
`__AdoptStyleSheet` — read at the call, so `__Card__`, `SystemInfo` and
`__globalProps` are the values the realm holds now. Plus this realm's
`globalThis`, which is how a chunk and the entry exchange anything. That is
what native does, where a chunk is a separate script evaluated in the same
context. A `var` at a chunk's top level is local to that call and declares no
global, and an `import` could not appear in a function body at all.

Queued jobs remain the enclosing checkpoint's work: a job a chunk queues runs
at the checkpoint the entry is already inside, not inside the load. ReactLynx's
`__LoadLepusChunk('worklet-runtime', ...)` caller is in
`packages/react/runtime/src/worklet-runtime/bindings/loadRuntime.ts` of the
read-only `lynx-stack` checkout.

`lynx.loadScript(key, {bundleName})` in this realm is the *same* synchronous
load with a different answer: a named custom section of whichever container
`bundleName` names, answered as its value — the module's default export, a
JSON body's parsed value, a `CommonJS` body's `module.exports` — rather than
run as a function body. A lazy container's `main-thread` section is one
expression, so what comes back is the function ReactLynx calls
(`lynx.loadScript('main-thread', {bundleName})(entry)`), and unlike a chunk it
is evaluated **once per realm**, because a URL is one module per realm.
`lynx.fetchBundle` is what installed that container; see
`docs/worker-resources-runtime.md` "Lazy containers". Data/update/reload
policy remains separate.

## Validation coverage

A real QuickJS boot test verifies synchronous processor/render ordering,
Promise identity, and the committed width at the flush microtask: the first
render job's width is committed while the nested job's later style mutation
remains dirty. Further tests cover processor/render errors and per-listener
error reporting without interrupting delivery or running jobs between listeners.

Chunk tests verify that a chunk is loaded and run once per call — two calls,
two host requests, two evaluations — that it was given the same runtime and
PAPI bindings as the entry, the absence of duplicate global bindings, that its
`var` does not leak, a chunk this page does not carry (its request refused), a
foreign component entry (no request at all), and deferred jobs. A
source-container integration test exercises selected entry/chunk registration
and on-demand evaluation through the shipped resource host. JavaScript tests
cover the URL built, the parameter list, the bindings the body receives, a load
failure, a body that throws, a body that does not compile, and the argument
checks. No compiled fixture artifacts are added.
