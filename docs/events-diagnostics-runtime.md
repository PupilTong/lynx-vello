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
  It snapshots the data at the call and returns `0` for an accepted peer send.
- The envelope contains only `type`, `data` and the sender's fixed `origin`:
  `CoreContext` from MTS and `JSContext` from BTS. Extra user properties cannot
  become runtime RPC control fields; there is no local echo.
- `postMessage(value)` sends a `message` event. An explicit `undefined` is valid;
  omitting the argument throws. Received null/undefined data is preserved.
- Listener registration/removal requires a string and function. Duplicate
  registrations collapse; capture/once options are ignored. Strict listeners
  receive `undefined` as `this`. Removal affects the active walk and additions
  wait for a later dispatch.

The existing tagged codec preserves undefined, special numbers and own data
properties across the JSON Worker channel. Context and runtime messages share
one FIFO, including sends before Worker connection. Named `callLepusMethod`
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

`LynxView::send_global_event(name, arguments)` uses the view's ordered `ToMain`
channel. Events arriving before the realm exists wait on the page. Once the
realm exists, its JS queue waits for the initial MTS render; boot marks that
point after the flush. The same Worker FIFO then delivers `sendGlobalEvent` to
BTS, which invokes its emitter. No JS callback crosses into Rust.

## Diagnostics

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
The `fatal` label does not cancel the view. Existing thrown-listener, Worker and
startup failures retain their separate error paths. SelectorQuery's error sink
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
Worker channel, including recovery after errors. The page-owner test holds an
entry import open and verifies global-event order across pre-realm, loading and
post-render commands.
