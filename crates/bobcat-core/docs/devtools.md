# bobcat-core: Devtools (CDP backend)

Part of the agent guide for `crates/bobcat-core`; the index
[crates/bobcat-core/AGENTS.md](../AGENTS.md) lists the other topics, and the
root [AGENTS.md](../../../AGENTS.md) holds the mission and the policies. Paths
in backticks are relative to `crates/bobcat-core/src/` unless they start with
`crates/` or `docs/`. The design and the user's rulings are
`docs/devtools-design.md`; this file says what the code in this tree does.

## What exists

Everything here is behind the cargo feature `devtools`
(`[features] devtools = ["serde_json/raw_value", "terminus/devtools",
"quickjs-rust-bridge/devtools"]`), which turns on the `devtools` features of
`terminus` (`Document::matched_styles`, `Document::set_node_highlight`) and of
`quickjs-rust-bridge` (value reflection, `function_with_values`). With the
feature off, no item, field, variant or branch of it is compiled. No embedder
crate enables it.

- **Public surface** (exported from the crate root):
  - `ViewSources::devtools: bool`, `false` in `ViewSources::new`. The runtime switch.
  - `LynxView::devtools_attach(&self, sink: Arc<dyn DevtoolsSink>) -> Result<DevtoolsSession,
    EngineError>`. Errors: `DevtoolsDisabled` (view built without the switch), `NotReady` (the
    view has ended, or its command channel refused the attach), `DevtoolsAttached` (a session of
    this view is alive). May be called before the view is ready.
  - `DevtoolsSession`: `Send`; `send(&self, message: String)`; dropping it detaches.
  - `trait DevtoolsSink: Send + Sync + 'static { fn message(&self, String); fn closed(&self); }`.
    `message` is called on the thread of the realm that answers (`bobcat-main` for the page,
    `bobcat-workers` for a worker). It must not block and must not call back into the engine.
    `DevtoolsSink::message` is called while the attachment's outlet is locked, which is what
    guarantees that no `message` call starts after `DevtoolsSession` has been dropped, and
    therefore the sink must not drop the session from inside `message`.
- **Domains answered**:
  - `Target.setAutoAttach`, and the events `Target.attachedToTarget` and
    `Target.detachedFromTarget`.
  - `Runtime.enable`, `Runtime.disable`, `Runtime.runIfWaitingForDebugger`, `Runtime.evaluate`,
    `Runtime.callFunctionOn`, `Runtime.getProperties`, `Runtime.releaseObject`,
    `Runtime.releaseObjectGroup`, and the events `Runtime.executionContextCreated`,
    `Runtime.consoleAPICalled` and `Runtime.exceptionThrown`.

  A worker session answers the `Runtime` methods only. Every other method of every domain
  (`Console.*` included) answers `-32601 '<method>' wasn't found`.

## Files

| File | Owns |
| --- | --- |
| `devtools/mod.rs` | `DevtoolsSink`, `DevtoolsSession`, `Outlet`, `WorkerRoutes`, `Attach` (the `ToMain::Devtools` payload), `Attachment`, `worker_session_id` |
| `devtools/protocol.rs` | `Request`, `Envelope`, `parse`, `session_id`, `Error`, `Frontend` |
| `devtools/target.rs` | `Target` domain, `AutoAttach`, `LiveWorker` |
| `devtools/runtime.rs` | `Runtime` domain: `dispatch`, `detach`, `RuntimeState`, `ConsoleTap`, `ConsoleCall`, the console install and hook; tests in `devtools/runtime/tests.rs` |
| `devtools/runtime/remote.rs` | RemoteObjects, previews, property descriptors, the object table (`Mirror`, `ObjectTable`, `RemoteObject`, `by_value`) |
| `devtools/worker.rs` | `WorkerDevtools`/`WorkerOp` (the `WorkerMessage::Devtools` payload), `WorkerSession`, `handle` |
| `main/runtime/devtools.rs` | `PageSession`, the page routing, the `MainThreadRuntime` methods |
| `devtools/test_support.rs` | `#[cfg(test)]` helpers every devtools test uses (below) |
| `devtools/tests.rs` | the attachment over a real view: refusal, one frontend at a time, the envelope's errors, worker sessions, the end of a view |

`main/runtime/devtools.rs` is a child module of the runtime because it reads
private fields there (`core`, `workers`).

## Message path

- `DevtoolsSession::send` runs on the caller's thread and parses only the top-level `sessionId`
  (`protocol::session_id`). A `sessionId` in the attachment's `WorkerRoutes` whose weak sender
  upgrades goes to that worker as `WorkerMessage::Devtools(WorkerDevtools { outlet, op:
  Message(text) })`. Everything else goes into the attachment's own unbounded channel to
  `bobcat-main`. The session never calls the sink.
- **Attach.** `ToMain::Devtools(Box<Attach>)` is sent once per attachment through the seat's
  counting `CommandSender`, so the frame-post fence counts it on both sides. `Page::apply_command`
  calls `MainThreadRuntime::devtools_attach`, which replaces any installed session, and spawns
  `serve_devtools` (`main/page.rs`) with `owner::spawn`.
- **`serve_devtools`** reads the channel and queues one `JobKind::Devtools` job per message with
  `owner::queue`, never awaiting it. When the channel closes (session dropped) it queues one more
  job, `devtools_end`.
- **Page job** (`MainThreadRuntime::devtools_message`): drops the message unless the installed
  session's outlet is the job's; parses the envelope; a `sessionId` here is `-32001`; then routes
  by domain: `Target` → `target::dispatch`, `Runtime` and `Console` → `runtime::dispatch`,
  anything else `-32601`.
- **Worker.** `consume_messages` (`background/thread.rs`) holds `WorkerMessage::Devtools` in order
  until the worker's root module has finished, as it holds `Post`, then queues one
  `JobKind::Devtools` job per message calling `devtools::worker::handle` with
  `WorkerRealm::devtools` and `WorkerRealm::core`. `Attach` creates the `WorkerSession` (replacing
  an earlier one), `Message` and `Detach` act only when their outlet is the session's. A worker
  session routes `Runtime` and `Console` to `runtime::dispatch` and answers everything else
  `-32601`, `Target.*` included. Its `Frontend` carries the worker's `sessionId`, which every answer
  and event has at the top level.

## Envelope and replies (`devtools/protocol.rs`)

Rust parses only the envelope (`id`, `method`, `params`, `sessionId`) and
writes the JSON of what Rust owns. JavaScript values are read by Rust through
the bridge's reflection entries: AGENTS.md "JavaScript data ownership" is
about host payloads handed to script, the opposite direction, and here Rust
needs the fields.

- `parse(text) -> Result<Envelope, (Option<i64>, Error)>`. Not JSON: `-32700`, no `id`. Not an
  object: `-32600 Message must be an object`, no `id`. `id` missing or not an integer: `-32600
  Message must have integer 'id' property`, no `id`. `method` missing or not a string: `-32600
  Message must have string 'method' property`, with the `id`.
- `Request { id: i64, method: String, params: Option<Box<RawValue>> }`; `domain()`;
  `params::<T: DeserializeOwned>() -> Result<T, Error>` (no `params` parses as `{}`; a failure is
  `-32602 Invalid parameters` with serde's reason as `data`).
- `Error::{method_not_found(method), invalid_params(data), server(message), session_not_found()}`:
  `-32601`, `-32602`, `-32000`, `-32001 Session with given id not found.`
- `Frontend` (cheap `Clone`, holds no `quickjs` value): `result(id, &T)`, `error(Option<id>,
  &Error)` (no `id` is an error notification), `reply(id, Result<T, Error>)`, `not_found(&Request)`,
  `event(method, &T)`, `is(&Arc<Outlet>)`, `outlet()`, `new(outlet, session_id)`. All JSON is
  written by `serde_json`.

## Domain interfaces

Each domain is one file under `devtools/`. A domain that needs more than one
file declares `mod` children of its own file (`devtools/runtime/remote.rs`),
and keeps its tests as a `#[cfg(test)]` child of its domain file
(`devtools/<domain>/tests.rs`). The page session (`main/runtime/devtools.rs`)
and the worker session (`devtools/worker.rs`) call the functions below. Every
dispatch answers through `frontend` itself, so it may answer later than it
returns.

`devtools/runtime.rs` (one implementation for the MTS realm and every worker realm):

```rust
#[derive(Default)]
pub(crate) struct RuntimeState(Option<Rc<Shared>>);
pub(crate) fn dispatch(state: &mut RuntimeState, core: &RealmCore, js: &mut ScriptRuntime,
                       frontend: &Frontend, request: &Request,
                       console_ready: bool) -> Result<(), ScriptError>;
pub(crate) fn detach(state: &mut RuntimeState, core: &RealmCore); // switches capture off
pub(crate) enum ConsoleCall { Log, Report }
#[derive(Default)]
pub(crate) struct ConsoleTap { /* on, installed, replaced, shared, unique_id */ }
impl ConsoleTap {
    pub(crate) fn is_on(&self) -> bool; // #[inline]
    pub(crate) fn message(&self, call: ConsoleCall, level: &str, text: &str);
    pub(crate) fn exception(&self, error: &ScriptError);
}
```

`core.engine` is the realm's `ScriptEngine`, `core.console` its `ConsoleTap`.
`dispatch` may run script. `console_ready` says that `bobcat:diagnostics` has
been evaluated in the realm; the error it returns is what the realm checkpoint
after the request's script left, which the caller reports under
`Scene::Devtools`. `devtools::worker::handle` takes `console_ready` as well and
returns that error; `MainThreadRuntime::devtools_message` returns it.

## Test helpers (`devtools/test_support.rs`)

- `booted(spec: TestViewSpec) -> TestEngine`: the spec with `with_devtools()`, booted.
- `attach(&TestEngine) -> (Arc<Recorder>, DevtoolsSession)`.
- `Recorder` (the sink): `count()`, `closed()`, `wait_for(what, Fn(&Value) -> bool)`,
  `wait_pumping(Option<&mut TestEngine>, what, Fn(&Value) -> bool)`, both answering the first
  recorded message that matches with the name of the thread it was sent on, and `answer(id)`.
- `Client::attach(&TestEngine)` (public fields `recorder`, `session`):
  - `call(&mut engine, session_id: Option<&str>, method, params: Value) -> Result<Value, Value>`
    sends one request with an id the client allocates (from 1 000 000), on the page session or on
    `session_id`, and waits for its answer, pumping the engine; `Ok` is `result`, `Err` the
    `error` object.
  - `result(..)` the same, failing the test with the error object on an error; `error(..)` the
    counterpart, failing on a result.
  - `event(&mut engine, session_id: Option<&str>, method) -> Value`: waits, pumping, for the
    first recorded event of `method` whose top-level `sessionId` is `session_id` (`None`: none)
    that this client has not returned before, and answers its `params`.

## Hook sites and their cost

| Site | Feature on, nothing attached | Attached |
| --- | --- | --- |
| `logScriptMessage` / `reportScriptError` (`realm/mod.rs`, `install_diagnostics`) → `ConsoleTap::message` | one `Cell<bool>` read per call | the same until `Runtime.enable`; then `ConsoleTap::message` |
| `policy::report` (`realm/policy.rs`), its first statement → `RealmOwner::console` → `ConsoleTap::exception` | per reported error: one `OnceCell` read (`Page::console`) or one field read (`Worker::console`), and one `Cell<bool>` read | the same until `Runtime.enable`; then `ConsoleTap::exception` |
| the realm's `Rc<ConsoleTap>`, which exists from realm creation on: `Worker::console` (created in `Worker::new`, handed to `RealmNames::console` in `Worker::boot`) and `Page::console` (set from `MainThreadRuntime::core().console` when `Page` stores its opened realm) | one `Rc` per realm and one field on `Page` and on `Worker`; no function registered; nothing per call | the console replacements, installed at the realm's first `Runtime.enable` |
| `WorkerOwner::start` (after the `Start` send succeeded, before `live.insert`) → `AutoAttach::worker_started` | one `RefCell` borrow and `Option` check | after `Target.setAutoAttach`: the announcement |
| `WorkerOwner::terminate`, `WorkerOwner::forget` → `AutoAttach::worker_ended` | one `RefCell` borrow and `Option` check | after `Target.setAutoAttach`: the announcement |
| the paint walk (`walk_within`, `crates/terminus/src/paint/walker.rs`) | one `Option` check (`Document::node_highlight`) | with a highlight set: the scan for the node's `ElementBox` item and up to four fills |

Nothing is added to tree mutation, the Element PAPI, the realm epilogue,
`LynxView::pump`, `ViewNotice`, `EngineEvent` or the `Painter`. Fields added:
`ViewSources::devtools`, `LynxView::devtools`, `MainThreadRuntime::devtools`,
`RealmCore::console`, `RealmNames::console`, `ShadowRealmHook::console`,
`ViewWorkers::devtools`, `WorkerRealm::devtools`, `Page::console`,
`Worker::console`, and in `terminus` `Document::node_highlight`. Trait method
added: `RealmOwner::console`. Variants added: `Scene::Devtools`,
`ToMain::Devtools`, `WorkerMessage::Devtools`, `JobKind::Devtools`
(`"devtools"`, Lynx task type `native_func_task`),
`EngineError::{DevtoolsDisabled, DevtoolsAttached}`, and in the bridge
`HostSlot::Values`, one more arm of the host-function dispatch `match`.
`ToMain` is at most 96 bytes and `WorkerMessage` at most 56 on 64-bit targets,
checked by a `const` assertion beside each, compiled with and without the
feature.

## Target domain

- `Target.setAutoAttach {autoAttach, waitForDebuggerOnStart, flatten}`: `flatten` not `true` is
  `-32602`. `autoAttach: true` (when off) lists every live worker of the view
  (`WorkerOwner::devtools_live_workers`: the page realm's `live`, then each `ShadowRealm` child's
  through `ViewWorkers::routes`, by key) and for each: inserts its weak sender in the route table,
  sends it `WorkerOp::Attach`, emits `Target.attachedToTarget {sessionId, targetInfo: {targetId,
  type: "worker", title, url, attached: true, canAccessOpener: false}, waitingForDebugger:
  false}`. Then stores `AutoAttach` in `ViewWorkers::devtools`, from which starts and ends are
  announced the same way (`Target.detachedFromTarget {sessionId, targetId}` on an end). `true`
  while already on does nothing more. `autoAttach: false` sends every routed worker
  `WorkerOp::Detach`, empties the table and emits `detachedFromTarget` for each.
- `sessionId` = `targetId` = `worker-<key>`. `url` is the worker's URL (`bobcat:bts` for the BTS);
  `title` is the URL.

## Runtime domain

Files: `devtools/runtime.rs` (dispatch, `RuntimeState`, `ConsoleTap`, console
install and hook), `devtools/runtime/remote.rs` (RemoteObjects, previews,
property descriptors, the object table), `devtools/runtime/tests.rs`. One
implementation serves the page session and every worker session.

- `RuntimeState` holds an `Rc<Shared>`, made at the session's first message. `Shared` holds the
  `Frontend`, the object table (a `RefCell`) and the exception counter. Only `RuntimeState` holds
  it strongly; it is dropped at detach and with the realm.
- `console_ready`: the page passes `true`, because a `MainThreadRuntime` is stored in the page
  only after `run_boot_module` returned `Ok`, and the boot module's static graph
  (`bobcat:runtime`, which imports `bobcat:diagnostics`) is evaluated by then. A worker passes
  `WorkerRealm::scope_installed`, set by `bobcat:worker`'s last statement after it imported
  `bobcat:diagnostics`. Nothing calls `module_namespace` while the flag is false.
- What a request's script leaves behind (`ScriptEngine::devtools_run` returns the result of
  `checkpoint_after_calls`, which discards the leftovers on a failure) is returned by `dispatch`
  and reported with `policy::report(.., Scene::Devtools, ..)` from `serve_devtools`
  (`main/page.rs`) and `deliver_devtools` (`background/thread.rs`). The row's event is the one
  of the table's `HostCall` row (`ScriptRunError` on the page, `WorkerPayload::Errored` on a
  worker), it does not end the owner, and its context is `in a devtools request`.
- Every script-running operation goes through `ScriptEngine::devtools_run`. Reflection reads run
  no script and use `ScriptEngine::devtools_realm`. Calls use the bridge's `Context::call`,
  `set_property` and `big_int_decimal`, which take `&self`.

Methods:

- `Runtime.enable`: if capture is on, answers `{}` and emits nothing. Otherwise installs the
  console replacements once per realm when `console_ready`, points the realm's `ConsoleTap` at
  the session (`Weak<Shared>`), switches capture on and emits `Runtime.executionContextCreated
  {context: {id: 1, origin: "", name: "", uniqueId, auxData: {isDefault: true, type:
  "default"}}}`. `uniqueId` comes from a process-wide counter and is fixed per realm. While capture
  is on and the install has not succeeded, every later message of the session tries it again.
- `Runtime.disable`: switches capture off and empties the object table.
- `Runtime.runIfWaitingForDebugger`: `{}`.
- `Runtime.evaluate`: runs `expression` as a classic script. `contextId` other than 1, or
  `uniqueContextId` other than the realm's, is `-32000 Cannot find context with specified id`.
  `awaitPromise: true` is `-32000`. `throwOnSideEffect: true` evaluates nothing and answers an
  `EvalError` object (`description: "EvalError: Possible side-effect in debug-evaluate"`) as
  `result` and as `exceptionDetails.exception`. `objectGroup`, `returnByValue` and
  `generatePreview` are honoured; other parameters are ignored.
- `Runtime.callFunctionOn`: evaluates `(<functionDeclaration>\n)` and calls it with `this` from
  `objectId` (or `undefined`). Arguments: `objectId`; `value` through `json_parse`; `NaN`,
  `Infinity`, `-Infinity`, `-0`; `<digits>n` through `big_int_decimal`; none means `undefined`.
  The object group defaults to the group of `objectId`. The same result handling as `evaluate`.
- A throw answers `result` plus `exceptionDetails {exceptionId, text: "Uncaught", lineNumber,
  columnNumber (0-based), url, exception, executionContextId: 1}`. The exception is an error object
  `{className: name, description: "name: message\nstack"}` built from the bridge's `Error`, or a
  string when the bridge reports no name; it has no `objectId`.
- `returnByValue` writes `value` with the intrinsic `JSON.stringify` for objects, functions and
  symbols, and answers `{type: "undefined"}` when there is no JSON text. A stringify failure is
  `-32000 Object couldn't be returned by value`.
- `Runtime.getProperties`: own properties through `own_properties` (`nonIndexedPropertiesOnly` is
  `OwnKeys::NonIndexed`). Without `ownProperties` it walks `prototype()`, marks `isOwn`, skips an
  inherited string key already seen and stops at a Proxy. A descriptor has `name` (for a symbol
  its `describe` text, plus `symbol`), `value` and `writable` or `get` and `set` (a missing half
  is `{type: "undefined"}`), `configurable`, `enumerable`, `isOwn`. `internalProperties`:
  `[[Prototype]]`, and for a promise `[[PromiseState]]` and `[[PromiseResult]]` (`undefined`
  while pending); none with `accessorPropertiesOnly`. A Proxy answers `{result: []}`.
- `Runtime.releaseObject`, `Runtime.releaseObjectGroup`. An `objectId` is a decimal string from a
  per-session counter; an unknown id is `-32000 Could not find object with given id` for every
  method that takes one.

RemoteObject rules (`remote.rs`, read without running script):

- `undefined`: `{type: "undefined"}`. `null`: `{type: "object", subtype: "null", value: null}`.
  Boolean and string: `value`.
- Number: `value` (an integer below 2^53 as a JSON integer) and `description` from `describe`;
  `NaN`, `Infinity`, `-Infinity`, `-0`: `unserializableValue` and `description`, no `value`.
  Bigint: `unserializableValue: "<digits>n"`, `description` the same.
- Symbol: `description` from `describe`, `objectId`.
- Function: `{type: "function", className: "Function", description: "function <name>()"}`, the
  name read as an own data property; `objectId`.
- Object: `subtype` from `ObjectClass` (`array`, `error`, `promise`, `proxy`, `typedarray`, `map`,
  `set`, `date`, `regexp`; none for a plain object), `className` (walk `prototype()`, read the
  data property `constructor` and its data property `name`, stop at a Proxy; else the class's own
  name, `Object` for a typed array), `objectId`. `description`: `Array(<own length>)`;
  `<className>(<length>)` for a typed array; `describe` (depth 1, strings cut to 100, 5 items) for
  an error, a date, a regexp, a map and a set; `Promise`; else the `className`. A Proxy:
  `{subtype: "proxy", className: "Object", description: "Proxy"}`, type by callability.
- `preview` (objects only; with `generatePreview` and always for console arguments): `type`,
  `subtype`, `description`, `overflow`, `properties`. An array or typed array: the first 100
  indices read one at a time with `own_property`, a hole skipped, `overflow` when the array's own
  `length` or the class's length is above 100; the other elements are not read. Other objects: up
  to 5 enumerable string-keyed own properties. Each property is `{name, type, subtype?, value}`
  with `value` a string cut to 100 characters or the description; an accessor is
  `{name, type: "accessor"}`. No `entries`; a Proxy has no properties.

Console capture:

- The install reads the `console` object exported by `bobcat:diagnostics` through the module
  namespace with `own_property`, and replaces each of `log`, `info`, `debug`, `warn`, `error`
  that is a data property holding a function. Each replacement is made by an engine-written
  factory script that uses syntax only, and calls one host hook (`ScriptEngine::devtools_function`)
  with `(level, original, this, args)`. The original is held by the JavaScript closure; the hook
  holds only `Rc<ConsoleTap>`.
- While capture is on, the hook records the arguments in object group `console` and emits
  `Runtime.consoleAPICalled {type, args, executionContextId: 1, timestamp}` (`type`: `log`,
  `info`, `debug`, `warning`, `error`; `timestamp` in milliseconds since the epoch from
  `clock::SystemTime`). It then calls the original with `Context::call` while
  `ConsoleTap::replaced` is above zero; during that call `ConsoleTap::message` ignores
  `ConsoleCall::Log`, so each call is reported once and `EngineEvent::ConsoleMessage` is
  unchanged.
- The replacements stay for the realm's life; after `disable` or detach they only call the
  original.
- A call through a method saved before the install reaches `logScriptMessage` and is reported as
  one string argument. `ConsoleCall::Report` is one string argument, type `warning` for level
  `warn` and `error` otherwise.

`Runtime.exceptionThrown`: `Page` and `Worker` keep the `Rc<ConsoleTap>` of their realm
(`RealmOwner::console`). `policy::report` tells that tap about every error it reports while
capture is on, before the row adds its context. The tap emits `Runtime.exceptionThrown
{timestamp, exceptionDetails}`: the exception is an error object whose `className` is the `Name`
of the message `"Name: message"` (`Error` when there is none) and whose `description` is the
message; the location comes from the `ScriptError`; the id comes from the session's exception
counter. The embedder's event is unchanged.

## Lifetime rules

- **One frontend at a time.** `LynxView::devtools` holds a `Weak<Outlet>` of the last session.
  Attach is refused while that outlet still has its sink. `DevtoolsSession::drop` takes the sink
  out of the outlet (without calling it) before the channel closes, so a new attach is legal the
  moment the drop returns.
- **Replacement.** An attach that reaches main while an earlier session is installed detaches the
  earlier one first. Every page job and the final `devtools_end` carry the outlet of the
  attachment whose task queued them and do nothing when it is not the installed one, so an earlier
  attachment's task never removes a later session and never runs against it. Worker operations
  carry the outlet too. No answer of an earlier attachment can reach a later sink: each
  attachment has its own outlet, and an earlier outlet's sink is gone.
- **`closed`.** `Attachment` (held by `PageSession`, or by an `Attach` not yet applied) closes the
  outlet on drop: the sink is taken and, if it was still there, `closed()` is called. It drops on
  `bobcat-main` with the realm's release, at detach, at replacement, or with a refused or
  discarded attach command, so `closed` is called once, on an engine thread, when the view ends
  or fails while the session is alive, and never after the session was dropped (the sink is
  already gone). An attach whose command send fails drops the session before the command, so
  nothing is called on the embedder thread.
- **No reference cycle.** `MainThreadRuntime::devtools` (declared first, so it drops before the
  realm) is the only strong handle on `PageSession`. A worker session is owned by `WorkerRealm`
  (declared before `core`). Host functions hold only the `ConsoleTap`, which holds no `quickjs`
  value. The route table holds weak senders, so a session that outlives its view keeps no worker
  channel open. Detach (`devtools_detach`): the runtime's `detach`, `target::detach` (auto-attach
  off, routed workers told to detach), then the session drops.
- **Re-entrancy.** Each domain's state on the page is its own `RefCell`. A `Runtime` request
  holds only the runtime state while script runs. The console tap and `AutoAttach` take no domain
  state.

## Not done

- Console messages and errors produced before `Runtime.enable` are not kept; nothing is replayed
  at enable.
- A child realm owner dropped with live workers (`WorkerOwner`'s `Drop`) announces no
  `detachedFromTarget`; only `terminate` and `forget` are hooked.
- `ShadowRealm` children share the parent's `ConsoleTap`; they are not separate execution
  contexts, and their console calls and errors are reported under context 1.
- `Runtime.evaluate` and `Runtime.callFunctionOn` do not support `awaitPromise` and do not
  evaluate with side-effect checks.
- An exception whose thrown value has no `name` is reported as a string, since the bridge's
  `Error` keeps only name, message, stack and location; `Runtime.exceptionThrown` carries no
  stack.
- A console call made through a saved method while a replaced method's original is joining its
  arguments is not reported.
- A Proxy's target and handler, and the entries of a Map or a Set, are not reported as
  structured data (no `entries` in a preview): without running script the bridge reads them only
  as `describe` text, and reading them as values needs a function in the QuickJS fork.
- No screenshot, screencast or input injection, and no transport: an embedder
  attaches through `LynxView::devtools_attach`, and none does yet.
- wasm32: `cargo check -p bobcat-core --features devtools --target wasm32-unknown-unknown`
  passes (2026-11-27). Nothing of the backend has run on wasm32, and `bobcat-wasm` does not
  enable the feature.
