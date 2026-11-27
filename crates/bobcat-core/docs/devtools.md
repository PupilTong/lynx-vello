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

  A worker session answers no method. Every other method of every domain answers `-32601
  '<method>' wasn't found`.

## Files

| File | Owns |
| --- | --- |
| `devtools/mod.rs` | `DevtoolsSink`, `DevtoolsSession`, `Outlet`, `WorkerRoutes`, `Attach` (the `ToMain::Devtools` payload), `Attachment`, `worker_session_id` |
| `devtools/protocol.rs` | `Request`, `Envelope`, `parse`, `session_id`, `Error`, `Frontend` |
| `devtools/target.rs` | `Target` domain, `AutoAttach`, `LiveWorker` |
| `devtools/worker.rs` | `WorkerDevtools`/`WorkerOp` (the `WorkerMessage::Devtools` payload), `WorkerSession`, `handle` |
| `main/runtime/devtools.rs` | `PageSession`, the page routing, the `MainThreadRuntime` methods |
| `devtools/test_support.rs` | `#[cfg(test)]` helpers every devtools test uses (below) |
| `devtools/tests.rs` | the attachment over a real view: refusal, one frontend at a time, the envelope's errors, worker sessions, the end of a view |

`main/runtime/devtools.rs` is a child module of the runtime because it reads
private fields there (`workers`).

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
  by domain: `Target` → `target::dispatch`, anything else `-32601`.
- **Worker.** `consume_messages` (`background/thread.rs`) holds `WorkerMessage::Devtools` in order
  until the worker's root module has finished, as it holds `Post`, then queues one
  `JobKind::Devtools` job per message calling `devtools::worker::handle` with
  `WorkerRealm::devtools`. `Attach` creates the `WorkerSession` (replacing an earlier one),
  `Message` and `Detach` act only when their outlet is the session's. A worker session answers
  every method `-32601`, `Target.*` included. Its `Frontend` carries the worker's `sessionId`,
  which every answer and event has at the top level.

## Envelope and replies (`devtools/protocol.rs`)

Rust parses only the envelope (`id`, `method`, `params`, `sessionId`) and
writes the JSON of what Rust owns.

- `parse(text) -> Result<Envelope, (Option<i64>, Error)>`. Not JSON: `-32700`, no `id`. Not an
  object: `-32600 Message must be an object`, no `id`. `id` missing or not an integer: `-32600
  Message must have integer 'id' property`, no `id`. `method` missing or not a string: `-32600
  Message must have string 'method' property`, with the `id`.
- `Request { id: i64, method: String, params: Option<Box<RawValue>> }`; `domain()`;
  `params::<T: DeserializeOwned>() -> Result<T, Error>` (no `params` parses as `{}`; a failure is
  `-32602 Invalid parameters` with serde's reason as `data`).
- `Error::{method_not_found(method), invalid_params(data), session_not_found()}`: `-32601`,
  `-32602`, `-32001 Session with given id not found.`
- `Frontend` (cheap `Clone`, holds no `quickjs` value): `result(id, &T)`, `error(Option<id>,
  &Error)` (no `id` is an error notification), `reply(id, Result<T, Error>)`, `not_found(&Request)`,
  `event(method, &T)`, `is(&Arc<Outlet>)`, `outlet()`, `new(outlet, session_id)`. All JSON is
  written by `serde_json`.

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
| `WorkerOwner::start` (after the `Start` send succeeded, before `live.insert`) → `AutoAttach::worker_started` | one `RefCell` borrow and `Option` check | after `Target.setAutoAttach`: the announcement |
| `WorkerOwner::terminate`, `WorkerOwner::forget` → `AutoAttach::worker_ended` | one `RefCell` borrow and `Option` check | after `Target.setAutoAttach`: the announcement |
| the paint walk (`walk_within`, `crates/terminus/src/paint/walker.rs`) | one `Option` check (`Document::node_highlight`) | with a highlight set: the scan for the node's `ElementBox` item and up to four fills |

Nothing is added to tree mutation, the Element PAPI, the realm epilogue,
`LynxView::pump`, `ViewNotice`, `EngineEvent` or the `Painter`. Fields added:
`ViewSources::devtools`, `LynxView::devtools`, `MainThreadRuntime::devtools`,
`ViewWorkers::devtools`, `WorkerRealm::devtools`, and in `terminus`
`Document::node_highlight`. Variants added: `ToMain::Devtools`,
`WorkerMessage::Devtools`, `JobKind::Devtools` (`"devtools"`, Lynx task type
`native_func_task`),
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
  (declared before `core`). The route table holds weak senders, so a session that outlives its
  view keeps no worker channel open. Detach (`devtools_detach`): `target::detach` (auto-attach
  off, routed workers told to detach), then the session drops.

## Not done

- A child realm owner dropped with live workers (`WorkerOwner`'s `Drop`) announces no
  `detachedFromTarget`; only `terminate` and `forget` are hooked.
- No screenshot, screencast or input injection, and no transport: an embedder
  attaches through `LynxView::devtools_attach`, and none does yet.
- wasm32: `cargo check -p bobcat-core --features devtools --target wasm32-unknown-unknown`
  passes (2026-11-27). Nothing of the backend has run on wasm32, and `bobcat-wasm` does not
  enable the feature.
