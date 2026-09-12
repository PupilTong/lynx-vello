# Worker resource loading

This stack layer adds native Script/JSON source reads to Worker realms,
including the built-in BTS Worker. It builds on the asynchronous ESM loader
and cancellation scope already required by the events/diagnostics and
readiness layer.

## Ownership and transport

`WorkerStart` carries a `SourceRequester` alongside its cancellation token.
The requester holds only the view's notice sender, its host wakeup, and that
worker's child token. It sends `ViewNotice::RequestSource` directly to the
host; `LynxView::pump` calls the existing `ResourceFetcher::request_source`.
The concrete `SourceCompletion` answers its oneshot receiver on the worker
thread. Neither source results nor JavaScript callbacks pass through MTS.
No new thread, executor, resource cache, or public callback registry is added.

The shipped native/Wasm fetcher handles `SourceRequest::Script` like other
text sources: URL resolution, transport, UTF-8 validation, and response URL
belong to the fetcher. Custom fetchers must handle the new enum variant and
return `LoadedSource::Entry`. Request locators are passed unchanged; native
module-name normalization belongs to the caller above this layer.

## ESM and top-level await

Worker realms enable the existing asynchronous QuickJS module loader and use
`start_module`, just as main does. Each normalized import URL starts one
request per realm. Responses retain the final URL as the base for static
and dynamic dependencies; duplicate imports share evaluation and namespace.
The existing QuickJS graph/linking and promise-job machinery is unchanged.

The entry can await imports, Script completions, and realm timers. Messages
posted before its evaluation settles remain in FIFO order. A worker's clock
and one task per outstanding load continue to enter and settle its realm.
Termination can end the worker while entry TLA is pending.

A handled import rejection stays in JS. An unhandled entry rejection reports
nonfatal worker errors and enables the message queue, preserving the
existing ordinary Worker failure contract. A rejection may be observed both
at the checkpoint that settles it and when entry status is read; this layer
preserves those error exits. A configured BTS entry instead
participates in the existing startup acknowledgement: the MTS failure binding
reports `StartupFailed`, and its ready binding precedes `ScriptFinished`/`is_ready`.
MTS evaluation completes independently of BTS readiness.

Raw BTS entries explicitly import their runtime bindings from
`bobcat:bts-runtime`. XML background entries now load through the host instead
of requiring test-only preloading.

## Native Script and JSON source text

`bobcat-internal:worker.readScript(path, timeout)` returns UTF-8 source text
synchronously. Finite positive timeouts are truncated to whole seconds;
nonpositive and nonnumeric values use five seconds, matching the MVP's native
read contract. An unrepresentable timeout throws before requesting a source.
A stylesheet result, fetch failure, dropped completion, timeout, or cancellation
throws into the calling JS stack.

This wait blocks the group's shared worker thread, including sibling worker
realms. It polls only the response and cancellation; it does not run a nested
executor, promise jobs, or timers. The embedder and MTS threads remain free.
The host must keep pumping resources. Releasing the view cancels the token
from the embedder thread and wakes the read immediately. A `terminate()`
message cannot interrupt a synchronous JS stack; it is consumed after the
read returns or times out.

The private `__BobcatRequestScript(path, callback)` export in
`bobcat:bts-runtime` registers a callback in JS, allocates its opaque ID, and
calls native `requestScript(id, path)`. Rust queues only that ID and locator.
A later completion calls `__BobcatCompleteScript(id, error, source)` in the
same realm. Success uses a null error; failure carries a string and empty
source. Callback cleanup runs even when the callback throws, which reports a
nonfatal worker error. A failed native request also removes the registration.
Independent completions may arrive out of order.

Both paths return text. JSON parsing, Script evaluation, compiled factory
execution, `lynx.requireModule`/`requireModuleAsync`, module caches, and lazy
bundle lookup remain in the subsequent module layer. The private asynchronous
request export isolates this transport PR from those dependencies.

## Cancellation and validation

Every load shares its worker's token. Worker termination ends pending async
loads; dropping the view cancels them and synchronous waits immediately.
Timed-out receivers are dropped, making `SourceCompletion::is_cancelled()`
true and discarding late results. A failed read can be retried in the same realm.

Worker tests cover redirected dependency graphs, duplicate imports, TLA timer
turns and message ordering, rejected entries, Script callbacks across realms,
retry, synchronous job ordering, timeout, and cancellation. JS tests cover
callback identity and cleanup. The resource integration test boots a real XML
BTS entry using ESM plus synchronous JSON and asynchronous Script sources,
then observes readiness and delivery of an MTS event queued during startup.
