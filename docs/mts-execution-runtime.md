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
`lynx.getEngine().dispatchEvent({ type: '__RenderPage', data, origin: 'Engine' })`.
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
results or drain jobs recursively inside a host callback. The existing outer
checkpoint continues to run jobs, report unhandled rejections and enforce its
budget/deadline. Entry-module and flush failures still fail boot. Later dirty
mutations retain the existing page epilogue's commit behavior.

Named cross-thread calls continue to await through Worker `postMessage` RPC.
Neither boot scheduling nor local chunk loading changes that boundary.

## Named Lepus chunks

`PageSource` preserves non-entry Lepus source chunks. It registers their source
map with `__BobcatRegisterLepusChunks` and supplies `source => eval(source)`
inside the selected entry module. That direct-eval closure retains the entry's
runtime/PAPI imports and lexical scope, without a global binding installer.

`__LoadLepusChunk` executes only a matching local card chunk, on demand and
again on each call. It returns false for an absent chunk or a different
component entry. Finding a chunk returns true even if its evaluation reports
an exception. Queued jobs remain the enclosing checkpoint's work. These lookup
and evaluation rules follow `core/runtime/lepus/bindings/renderer_functions.cc`
and `core/renderer/template_entry.cc` in the read-only native Lynx checkout.

The retained entry scope lets a chunk access the module's own variables;
it does not provide arbitrary global Script declarations shared between
separate evaluations. ReactLynx's `__LoadLepusChunk('worklet-runtime', ...)`
caller is in `packages/react/runtime/src/worklet-runtime/bindings/loadRuntime.ts`
of the read-only `lynx-stack` checkout. Public `lynx.loadScript`/`fetchBundle`,
complete lazy-container loading and data/update/reload policy remain separate.

## Validation coverage

A real QuickJS boot test verifies synchronous processor/render ordering,
Promise identity, and the committed width at the flush microtask: the first
render job's width is committed while the nested job's later style mutation
remains dirty. Further tests cover processor/render errors and per-listener
error reporting without interrupting delivery or running jobs between listeners.

Chunk tests verify retained entry imports, absence of duplicate global bindings,
repeat evaluation and deferred jobs. A source-container integration test
exercises selected entry/chunk registration through the shipped resource host.
JavaScript tests cover local/missing chunk lookup and repeated failed evaluation.
No compiled fixture artifacts are added.
