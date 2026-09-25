# Runtime events and diagnostics

This stack layer implements native Lynx Context behavior, the BTS
GlobalEventEmitter, and nonfatal console/error delivery to the owning host.
It uses the existing Worker transport and requires no compiled bundle loader,
new thread, vendor change or Rust-side JavaScript callback registry.

## Context directions and values

MTS `lynx.getJSContext()` and BTS `lynx.getCoreContext()` are stable
`CrossThreadContext` instances. They share EventTarget's listener storage and
walk, with Lynx-specific argument and receiver semantics:

- `dispatchEvent` requires an object with a string `type` and a `data` property.
  It captures the public envelope and returns `0` for an accepted peer send.
- The envelope contains only `type`, `data` and the sender's fixed `origin`:
  `CoreContext` from MTS and `JSContext` from BTS. Extra user properties cannot
  become runtime RPC control fields; there is no local echo.
- `postMessage(value)` sends a `message` event. An explicit `undefined` is valid;
  omitting the argument throws. Received null/undefined data is preserved.
- Listener registration/removal requires a string and function. Duplicate
  registrations collapse; capture/once options are ignored. Strict listeners
  receive `undefined` as `this`. Removal affects the active walk and additions
  wait for a later dispatch.
- The shared `bobcat:event-target` EventTarget these Contexts, the `Worker`
  objects and the engine target all extend follows the DOM's inner-invoke rule:
  a listener that throws is reported and the walk continues with the next
  listener. In the MTS realm the report goes through `lynx.reportError` to the
  host's `reportScriptError`, as a nonfatal `EngineEvent::ScriptReported`; in a
  worker realm it goes through the worker global's `reportError`, so it reaches
  the parent `Worker`'s `error` event and a nonfatal `WorkerThrew`.

Context and runtime messages share one FIFO, including sends before Worker
connection. Queued payloads remain references until Worker `postMessage` copies
them as a structured clone. Undefined object members, undefined array entries,
the nonfinite numbers, negative zero, `BigInt`, `Date`, typed arrays, cycles and
shared references all survive; `toJSON` is never consulted, because structured
clone has no such hook. A value the serializer refuses — a function, a `Symbol`,
a `Map`, `Set`, `RegExp`, `Error` or `DataView` — throws at the call. No custom
codec or compensating deep clone sits on top of the transport.
Named `callLepusMethod`
RPC retains the stack's selected web-worker-rpc async boundary.
MTS `getCoreContext()` and `getNative()` remain inactive direction sinks.

## Global events

BTS `lynx.getJSModule('GlobalEventEmitter')`,
`lynx.getApp().getJSModule('GlobalEventEmitter')` and
`lynx.getApp().GlobalEventEmitter` return the same instance. `registerModule`
and `getJSModule` share the app's module table. The MTS event-module shell
remains inactive, as before.

This emitter is an argument-list bus, separate from Context and DOM events.
It retains duplicate registrations and optional receivers, removes the first
matching listener, and iterates the live array. `emit`/`toggle` spread arguments;
`trigger` passes one payload and parses JSON strings only when listeners exist.
A listener throw stops that emission; later Worker messages still run.

`LynxView::is_ready()` becomes true when `pump()` observes `ScriptFinished`,
which means MTS boot finished: the entry module evaluated, its top-level await
settled, and its first flush committed. An entry that threw finishes boot too:
its failure is reported first, as a nonfatal `ScriptRunError`, and boot goes on
to render and flush. The BTS Worker's state — still
importing its entry, its entry threw, or it ended — is no part of that, so a
BTS entry whose top-level await never settles does not keep the view from
becoming ready.
A BTS entry that throws calls the worker realm's `reportError`, so it reaches
the `Worker`'s `error` event and a nonfatal `WorkerThrew` from
`ScriptSource::Background` like any other worker script; BTS stays up and
still takes messages. A BTS that ends without being told to — its realm could
not be built, or the worker thread trapped — is a nonfatal `WorkerEnded` from
the same source. No BTS failure ends the view.
Worker ESM loading is part of this layer: the bootstrap's application import
requests its source from the view fetcher, including XML background entries.
The first Worker message initializes BTS before that import; later messages
wait on its Promise while imports and timers continue.
The remaining [compiled-module loading contract](worker-resources-runtime.md)
is defined by ReactLynx callers, without lynx-core's source-text read APIs.

`LynxView::send_global_event(name, arguments)` takes an embedder-serialized JSON
array `String` for `arguments`. Core passes it unchanged to MTS JS, which parses
it and constructs the Worker message. The method returns `EngineError::NotReady`
before `pump()` observed MTS boot finishing or after the view ends; the BTS
still importing its entry is not a reason to refuse one. Rejected events are not
retained or replayed. Accepted events use the ordered `ToMain` channel and
Worker FIFO to deliver `sendGlobalEvent` to the BTS emitter. There is no page
or JS queue for early host global events, and no JS callback crosses into Rust.
The internal pre-connection queue remains for messages generated by the MTS
entry itself, which necessarily runs before boot constructs its Worker; it is
the only MTS-side queue. MTS keeps its Worker reference after that Worker ends,
and a post to an ended Worker is dropped by the host, as a browser drops
`postMessage` to a terminated worker.

## Diagnostics

Worker failures remain typed Rust diagnostics for the host: `WorkerThrew` for
a worker that threw and still runs, `WorkerEnded` for one that ended without
being told to, each carrying the worker's `ScriptSource` and reported before
the JS `error` event is dispatched. MTS receives the message, filename, line
and column as primitive binding arguments and creates the Worker error event
in JS; Rust builds no diagnostic envelope of its own.

Both runtimes export `console.log/info/debug/warn/error`. MTS entries receive
`console` through their injected ESM import; raw BTS entries can import it from
`bobcat:bts-runtime`. Installing the compiled BTS wrapper environment is a later
stack layer. These exports add no global-object properties.

`lynx.reportError` and MTS `_ReportError` normalize severity to `error`, `warning`
or `fatal`. Error values retain their message and stack; other values use JSON
when possible and string conversion as fallback. Console arguments use the
same formatting and are joined with spaces. This is the extracted MVP console
surface, without formatting directives, grouping, inspection or native `alog`.

BTS sends tagged `reportError`/`console` messages via `globalThis.postMessage`.
MTS handles those messages in JS and calls `reportScriptError`/`logScriptMessage`.
The host callbacks publish `EngineEvent::ScriptReported`/`ConsoleMessage` through
`ViewOutbox`; `LynxView::pump` exposes them to the embedder. CLI/headless/server
hosts forward them to stderr, and the browser host forwards them to its console.
The `fatal` label does not cancel the view. Worker, DOM-listener
(`ListenerFailed`) and startup failures retain their separate error paths; a
thrown JS `EventTarget` listener reports through this same reporter while its
dispatch continues. SelectorQuery's error sink
now uses this reporter while retaining its existing status replies.

## Evidence and checks

Native reference paths in the read-only `lynx/` checkout:

- `core/runtime/js/bindings/event/context_proxy_in_js.cc` and
  `core/runtime/js/bindings/event/js_event_listener.cc`: argument validation
  and listener receiver.
- `core/runtime/common/bindings/event/context_proxy.cc` and
  `core/runtime/common/bindings/event/message_event.cc`: origin and message
  operations.
- `js_libraries/lynx-core/src/modules/event/eventEmitter.ts`: duplicate
  registrations, receivers, live-array iteration and trigger payloads.

JS tests cover Context values/validation, emitter mutations and diagnostic
formatting/forwarding. Real QuickJS tests exercise both realms and the actual
Worker channel, including recovery after errors. Public view tests cover
readiness observed through `pump()`, startup failure, an entry that throws and
still becomes ready, early-event refusal without replay, and refusal after
cancellation. Page-owner and runtime tests verify that
MTS boot alone settles readiness, and that a BTS failure reports one
`WorkerEnded` from `ScriptSource::Background` and still publishes
`ScriptFinished`, without a `StartupFailed`, a `WorkerThrew` or a listener
failure. Runtime tests fix which of the two worker events a throw and an end
report, the source each names, and that a worker stopped with `terminate()`
reports neither.
