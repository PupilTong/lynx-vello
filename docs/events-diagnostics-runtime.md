# Runtime events and diagnostics

This stack layer implements native Lynx Context behavior, the BTS
GlobalEventEmitter, and nonfatal console/error delivery to the owning host,
which every realm makes itself, named by its source. It uses the existing
Worker transport and requires no compiled bundle loader, new thread, vendor
change or Rust-side JavaScript callback registry.

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
  host's `reportScriptError`, as a nonfatal `EngineEvent::ScriptReported` from
  `ScriptSource::Main`; in a worker realm it goes through the worker global's
  `reportError`, so it reaches the parent `Worker`'s `error` event and a
  nonfatal `WorkerThrew`.

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
still takes messages. An animation-frame, `queueMicrotask` or
`lynx.fetchBundle` callback of the BTS that throws is reported the same way.
A BTS that ends without being told to — its realm could not be built, or the
worker thread trapped — is a nonfatal `WorkerEnded` from the same source,
once: a trap after that end was delivered is reported to no one. No BTS
failure ends the view, and neither does a `lynx.reportError` of any level.
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

Every realm has `console.log/info/debug/warn/error` and `lynx.reportError`,
and they are one module, `bobcat:diagnostics`, in all three realm kinds. MTS
entries receive `console` and `_ReportError` through their injected ESM
import. The BTS exports `console` from `bobcat:bts-runtime`, where a raw BTS
entry imports it and a bundle body's preamble binds it. A worker realm, the
BTS included, also has `console` on its global, installed by `bobcat:worker`
as WebIDL installs a namespace: writable, configurable and not enumerable. It
is the same object the BTS module exports. A plain `Worker` has no global
`requestAnimationFrame`; it imports one from `bobcat:animation-frame`, the
module `bobcat:bts-runtime` also takes its export and `lynx` member from. The
MTS realm adds no global `console`.

The module is written over two members every realm's core has under
`bobcat-internal:host`, `reportScriptError(level, message)` and
`logScriptMessage(level, message)`. Each sends one
`EngineEvent::ScriptReported` or `ConsoleMessage` to the view's host through
the realm's own `HostOutbox`, carrying the realm's `ScriptSource` — `Main`,
`Background`, or `Worker(WorkerId)` for a `Worker` the MTS script
constructed — from whichever thread the realm is on. No realm relays another
realm's diagnostics: a BTS or `Worker` diagnostic reaches the host while MTS is
busy, and it is not a Worker message the MTS realm dispatches. One realm's
diagnostics arrive in the order it made them; two realms' have no order
between them, and a worker's diagnostics have none relative to its
`WorkerThrew` and `WorkerEnded`, which the creating realm reports.
`LynxView::pump` exposes them to the embedder.

A `ConsoleMessage` level is the console method's name. `lynx.reportError(error,
{level})` reports at lynx-core's levels spelled as the console methods would
spell them: `'warning'` is `"warn"`, `'fatal'` is `"fatal"`, and `'error'`, an
unknown level and no level are `"error"`. MTS `_ReportError` is the same
function as its `lynx.reportError`. A `"fatal"` report is a diagnostic like
the others: the engine does not cancel the view and does not stop the BTS from
taking calls, where native stops its BTS. Error values retain their
message and stack; other values use JSON when possible and string conversion
as fallback. Console arguments use the same formatting and are joined with
spaces. This is the extracted MVP console surface, without formatting
directives, grouping, inspection or native `alog`.

A report is not an exception. `lynx.reportError` never reaches a `Worker`'s
`error` event, and a worker exception never becomes a `ScriptReported`: in a
worker realm an uncaught exception, a listener that throws and the BTS's
animation-frame, `queueMicrotask` and `lynx.fetchBundle` callbacks that throw
all go through the global scope's `reportError`, HTML's path, to the parent
`Worker`'s `error` event and `WorkerThrew`. Two BTS paths stay on
`lynx.reportError`: the disposal hook's throw, which has to reach the host
before the `disposed` reply lets MTS terminate the Worker and drop anything
the Worker reports after it, and a misused `SelectorQuery`, whose error sink
reports while retaining its existing status replies. DOM-listener
(`ListenerFailed`) and startup failures retain their separate error paths.

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

JS tests cover Context values/validation, emitter mutations, diagnostic
formatting and levels, and that the BTS sends no diagnostic to the main
thread. Real QuickJS tests exercise both realms and the actual Worker channel,
including recovery after errors, and fix the source each realm's diagnostics
carry: MTS and BTS reports and console output, a plain `Worker`'s global
`console`, BTS frame and microtask callbacks that throw as `WorkerThrew`, and
a misused `SelectorQuery` as a `ScriptReported` from the BTS. Public view
tests cover readiness observed through `pump()`, startup failure, an entry
that throws and still becomes ready, early-event refusal without replay, and
refusal after cancellation. Page-owner and runtime tests verify that
MTS boot alone settles readiness, and that a BTS failure reports one
`WorkerEnded` from `ScriptSource::Background` and still publishes
`ScriptFinished`, without a `StartupFailed`, a `WorkerThrew` or a listener
failure. Runtime tests fix which of the two worker events a throw and an end
report, the source each names, and that a worker stopped with `terminate()`
reports neither.
