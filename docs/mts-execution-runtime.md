# MTS calls and named Lepus chunks

This layer adds on-demand execution of local named Lepus chunks and the
same-thread Promise-job/error policy for MTS functions and native engine
listeners. Bundle lookup/cache, the public `lynx.loadScript` API,
data/update/reload policy and compiled BTS bootstrap remain separate layers.

## MTS calls and errors

The native call audit used Lynx revision
`66b002855a25a5a8812fe878af69e20a346d0408` and Lynx 4.1.0. Relevant owners are
`core/runtime/lepusng/quick_context.cc` (`InternalCall`) and
`core/runtime/lepus/bindings/event/lepus_event_listener.cc`
(`LepusClosureEventListener::Invoke`).

After a successful function call, `callMts` drains Promise jobs before returning
its original result. A synchronous throw reports and returns undefined without
that nested drain; the enclosing checkpoint owns the jobs it left pending.
A failed job or exhausted job budget reports and discards the result.
Unhandled Promise rejections report without replacing a successful result.
A returned Promise remains the same Promise; this call does not await it.

`callMts` uses the global object as receiver. Native engine dispatch wraps each
listener separately, so one listener's successful checkpoint finishes before
the next listener; a failed listener does not stop the walk. Ordinary
`EventTarget.dispatchEvent` retains its existing JavaScript semantics.

Boot applies this policy to `processData`, `renderPage`, and the fallback
`__RenderPage` engine event. A hook error reports without failing entry boot;
an entry-module error still fails startup. The existing hook selection and
payload shape remain unchanged. Named cross-thread calls continue to await
through Worker `postMessage` RPC; Rust does not dispatch application hooks.

## Named Lepus chunks and imported bindings

`PageSource` preserves non-entry Lepus source chunks. It registers their source
map with `__BobcatRegisterLepusChunks` and supplies `source => eval(source)`
inside the selected entry module. That direct-eval closure retains the entry's
runtime/PAPI imports and lexical scope. It needs no global binding installer,
second copy of those bindings, or native Script evaluator.

`__LoadLepusChunk` executes only a matching local card chunk, on demand and
again on each call. It returns false for an absent chunk or a different
component entry. Finding a chunk returns true even if its evaluation reports
an exception. Chunk evaluation does not add a nested checkpoint: queued jobs
belong to the enclosing checkpoint. These lookup/evaluation rules follow
`core/runtime/lepus/bindings/renderer_functions.cc` and
`core/renderer/template_entry.cc`.

The retained entry scope lets a chunk access the module's own variables;
it does not provide arbitrary global Script declarations shared between
separate evaluations. ReactLynx's `__LoadLepusChunk('worklet-runtime', ...)`
caller is in `packages/react/runtime/src/worklet-runtime/bindings/loadRuntime.ts`
of the read-only `lynx-stack` checkout.

## Checkpoints and lifetime

The bridge exposes a weak, owner-thread `JobQueue` handle and a separate `Fn`
reentrant host callback registration. The callback cannot retain its own realm
through that handle. Existing `FnMut` callbacks still refuse reentry. Core owns
the finite job budget, error reporting and checkpoint-generation notification;
the enclosing execution deadline still applies. Nested drains execute only JS
jobs, not timers, resource work, renderer commits or a nested executor.

The queue is runtime-wide, so sibling-realm jobs may run. Unhandled rejections
remain attributed to their own realm, and the generation notification lets a
sibling settle work completed by another realm's checkpoint. There is a single
stop-on-failure job policy; no extra continuation mode exists for native Script
evaluation.

## Validation coverage

Bridge tests cover nested/reentrant jobs, bounded work, rejection isolation and
weak callback lifetime. Core tests cover processor-to-render ordering, original
Promise/object results, nonfatal reports, per-listener checkpoints, and nested
calls from a Promise job. The chunk test verifies that runtime/PAPI values are
module imports, absent even from the global lexical environment, and remain
available to the chunk through its entry scope. It also checks repeat evaluation
and the enclosing checkpoint's ownership of chunk jobs.

A source-container integration test exercises selected entry/chunk registration
through the shipped resource host. JavaScript tests cover missing/local chunk
lookup and repeated failed evaluation. No compiled fixture artifacts are added.
