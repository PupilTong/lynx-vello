# bobcat-core: Workers, the BTS and cross-thread messages

Part of the agent guide for `crates/bobcat-core`; the index
[crates/bobcat-core/AGENTS.md](../AGENTS.md) lists the other topics, and the
root [AGENTS.md](../../../AGENTS.md) holds the mission, the standards and
dependency policies, the workspace map, the toolchain and testing. Paths in
backticks are relative to the repository root.

**A group also owns a second thread and a second `QuickJS` runtime,
`bobcat-workers`**, for the worker realms every view in it shares.
`LynxGroup::new` starts it beside `bobcat-main`, and the group handle's drop
joins it after `bobcat-main` has returned, so a thread that will not start
fails the *group*. `bobcat-main` holds one sender on it and only ever sends:
start a context, post to a context, stop a context — and hears
events back. It also reads one flag the worker thread sets when it traps: the
trap reports `Failed` to the creator of every worker still on it, and a
`new Worker` constructed afterwards fails at once instead of being sent there.
A released view stops its own workers by sending each that stop.
The price is one parked thread and one idle runtime per group; there is no
lazily-built state and no lock. The runtime is separate from `bobcat-main`'s
because worker script must not stop the thread that owns the document, and
since `QuickJS` binds a runtime to one thread, no path runs from a worker realm
to a `LynxDocument` and no value of either runtime can be named by the other.
One realm per live worker, and the group's workers take turns. **One task per
live worker, and a worker's whole state is that task**: a `WorkerStart` carries
its key, its name, its URL, the receiving end of its message channel, the
sender its events go back on — the view's one `WorkerEvent` channel, which
every realm over the view's document reports on — the Worker's own cancellation token, and the `HostOutbox` it asks the
view's host through. It carries neither a script nor a `ScriptSource`: the
worker's realm asks for its own script, and both threads write the source
from the key and the URL, as `ScriptSource::Worker { id, url }`. There is no
worker kind: the BTS is the
dedicated worker whose URL is `bobcat:bts`, and nothing of the view's data is
in its `Start`: that reaches it in the `initialize` message. The
worker's realm opens as its `Start` is served, the way a view's opens as that
view's first job, and **a worker's root module is the module at its URL**:
the realm loads that module as its root, the way `import(<URL>)` loads one
(`ScriptEngine::load_root_module` over the bridge's `Context::load_module`),
with nothing written around it, so the BTS's root module is `bobcat:bts`
itself and a plain worker's is its fetched script. A runtime that never came
up fails the worker there, before anything is requested. **The engine
installs no global scope in a worker realm.** `bobcat:bts` begins with
`import "bobcat:worker"; import "bobcat:timers";`, and a plain worker script
that wants `self`, `postMessage`, `onmessage`, `close`, `name` or `console`
imports `bobcat:worker`, and one that wants the timer globals imports
`bobcat:timers`; a script that uses them without the import throws a
`ReferenceError`, and nothing guards against it (the engine itself
constructs only the BTS; a plain `Worker` comes from MTS code that imports
`bobcat-internal`, or from tests). A post is delivered through
`bobcat:worker`, so a realm in which that module has not run drops what is
posted, reporting nothing. The test is the module having *run*, which the
worker thread learns from `bobcat:worker`'s one read of `workerName`, its last
statement (`WorkerFlags::scope_installed`), not the realm having an instance
of it: a
graph that failed to load or is still loading leaves its modules compiled but
never linked, and reading the namespace of such a module crashes QuickJS.
MTS routes events through weak references to JS Worker objects; their
finalizers and explicit `terminate()` release sending handles, and releasing
the MTS realm closes its remaining senders. **Every realm over the view's
document has a `WorkerOwner` of its own** — the page realm and each of its
`ShadowRealm` children (`main/workers.rs`) — and all of them share the view's
`ViewWorkers`: the one events channel, and a table from the key of each
worker a child created to that child's owner. `dispatch_worker_event` reads
it first, so an event reaches the realm whose `Worker` object it is for: a
key no child recorded is the page realm's, and an event for a child that is
gone is dropped (`ChildRealms::call_module_export`). A child's owner is its
`ChildHostState`, dropped with the child after its realm, which closes its
workers' channels and takes its keys out of the table. Apart from those
handles, the realm's `WorkerOwner` records each key's public `ScriptSource`
(`Worker { id, url }`: the key's id and the worker's script URL, the value the
worker thread writes too, from the same `Start`) from the moment the key is
allocated until
`terminate()` or delivery of the worker's own end, so a worker that fails
before it is started has a source too; `WorkerThrew` and `WorkerEnded` carry
it, and a key without one reports neither — so a trap that reaches a worker
after its own end was delivered is reported to no one. Host functions
reference the channel owner weakly, so queued finalizers cannot keep a
released realm's workers or group thread alive. A worker's script is completed
by a job, as any module it imports is, and a job of a worker that has ended
does nothing: a `terminate` the worker's message consumer has read before
that job runs means the script never runs, and nothing else orders the two.
The consumer starts beside the
worker's first job rather than after it, so a `terminate` also ends at once a
worker whose first job is still queued behind another realm's parked job;
that job then opens no realm and requests nothing. A
worker whose URL is an engine name raises no request: the
realm's own loader loads a registered name, and refuses any other with a local
`ReferenceError`, which the worker reports as `WorkerThrew` and keeps running.
The timer machinery both
realm kinds run on — the schedule, the two host members, the firing loop — is
`crate::timers` beside `crate::clock`, owned by neither thread.

**The main-thread `Worker` class is exported by `bobcat-internal`.** It is an
explicit ESM import, creates a distinct context on the group's existing
`bobcat-workers` thread, and supports `postMessage`, `terminate`, `onmessage`,
`onerror` and the shared EventTarget listener methods. It uses module scripts
(also with omitted options) and the worker scope's structured-clone transport,
so `undefined`, `NaN`, `Date`, `BigInt`, typed arrays, cycles and shared
references survive, while a function, `Symbol`, `Map`, `Set`, `RegExp`,
`Error`, `DataView` or accessor property throws synchronously at the
`postMessage` call; transfer lists remain pending. External ESM imports load
through the view's resource fetcher and support TLA. `main/workers.rs` installs
its three native operations — `createWorker`, `sendWorkerMessage`,
`terminateWorker` — before entry boot. `createWorker` joins every specifier by
URL rules to the `__Card__` the realm passes as its third argument — a
container page's container URL, otherwise the entry's response URL (Rust does
not keep `__Card__`; an absolute URL such as `bobcat:bts` joins to itself); a
URL that does not resolve starts nothing and `new Worker` throws a synchronous
`SyntaxError`. `createWorker` asks the host for nothing and checks no prefix.
The worker's realm loads the module at its URL as its root when it boots; for
a URL the realm has no source for, that load is one module request, which the
worker's epilogue sends through its `HostOutbox` as a `SourceRequest::Module`
and a task of the worker completes as it does any import's: under the request
URL, from the response URL, as a module of its own realm, once, without a
main-thread turn. An engine name is loaded or refused by the realm's loader
and never requested. A script that cannot be loaded, or is answered with
something other than a script, is reported once as `WorkerThrew`, under
`loading a worker module`, the one context every completion of a module
reports under, and the worker stays until it is terminated or collected
(`docs/tracking/deviations.md`). Posted
messages wait until the root module's load has settled. `self.name` is set by
`bobcat:worker` from the host member `workerName` as it is evaluated, so a
module the script imports statically after it reads it too. Every concurrent
worker request is preserved.
Worker entry/import requests use the Worker's cancellation scope, and host
release does not cancel it ahead of JS disposal. Once the MTS realm is
released, closing its senders ends remaining Workers, including after failed
boot; Rust has no Worker termination sweep. A `WorkerEvent` delivers messages
and errors to the owning realm; a worker's `Errored` also produces a nonfatal
`EngineEvent::WorkerThrew` and its `Failed` a nonfatal
`EngineEvent::WorkerEnded`, each naming the worker by the `ScriptSource` its
`WorkerOwner` recorded. See `docs/runtime-architecture.md` for the
transport and lifetime boundaries.

**After the MTS entry import settles, whether the entry succeeded or threw,
boot creates a BTS Worker** named
`lynx-bg` through that same class, using the engine URL `bobcat:bts`. The BTS
is a dedicated worker like any other, started the same way, and Rust has no
check that tells it apart: its URL is the `url` of its `ScriptSource`, as
every worker's is. That URL is a registered module (`bts.ts`) and the BTS
realm's root module: it imports `bobcat:worker` and `bobcat:timers` (the
BTS's global scope and timers), installs its JS initializer from
`bobcat:bts-runtime` and returns, with no top-level `await`, so the first
message is delivered. That message, `initialize`, which
`__BobcatConnectBackground` posts as boot connects the Worker, carries the
page data and the rest of what the BTS starts with: the
`ViewSources.background_entry` URL (written into the MTS boot module as a
JSON literal, `undefined` when the view named none), the MTS realm's own
`SystemInfo`, and the view's native module table (the MTS startup member
`nativeModuleTable`, posted unread). `bobcat:bts-runtime` fills `SystemInfo`
(its live export, `lynx.SystemInfo` and the global) and `NativeModules` from
it, and then `bobcat:bts` imports the entry it names. A plain `Worker` that
imports `bobcat:bts-runtime` is posted no `initialize` and keeps the runtime
constants and an empty `NativeModules`. MTS JavaScript owns BTS disposal: send
`dispose`, await `disposed`, then terminate its Worker. BTS calls the current
app hook, reports any throw and replies after an ordinary Promise boundary. The
MTS disposal Promise also handles repeated destroy notifications, and disposal
bypasses an unfinished BTS entry import. Object observers follow web-core: a
plain object registered with a JS `FinalizationRegistry` that directly invokes
its callback (`docs/destruction-runtime.md`). A raw BTS application entry
imports its bindings from `bobcat:bts-runtime` itself, `lynx` included. An XML
page's background-thread script is a card body instead: `bobcat-source`
registered it after `BTS_CHUNK_PREAMBLE`, so it has `lynx` and the rest of that
list without importing them, and must not import any of them (a second binding
is a `SyntaxError`). Neither runtime installs `globalThis.lynx`. XML uses this
identical startup path, and the bootstrap contains no application source and
does not fetch it in advance. A
worker carries a `HostOutbox` (`WorkerStart.sources`) that sends module
requests, its own script's included, directly to the view's resource host.
ESM completion and timers
continue during entry TLA; posted messages wait for entry settlement, and each
completion shares its worker's cancellation token. ReactLynx compiled module
execution (`lynx.requireModule` and the `{init}` factory ABI in
`bobcat:lynx-modules`) and the lazy-bundle APIs (`lynx.fetchBundle`,
`lynx.loadScript`) are a layer over this transport; bypassing `lynx_core.js`
does not require its `requestScript`/`readScript` source-text interfaces
(`docs/worker-resources-runtime.md`). Without an entry, only the built-in
environment runs. All workers use the same protocol; a worker's scope is what
its script imports.

**`lynx.setSharedData`/`getSharedData` and the shared-data observers are
lynx-core's, over the runtime's shared object** (user ruling 2026-10-15).
Native and web-core keep `nativeGlobal.sharedData` and the
`ShareDataSubject` on the background JS global a group's cards share; here
each view's BTS is a realm of its own on the group's worker runtime, and
`bobcat:bts-runtime` keeps both on `bobcat-internal:worker`'s `sharedObject`
instead (see `crates/quickjs-rust-bridge/AGENTS.md`), which every worker realm of the group
holds by reference. The first BTS to run makes the null-prototype store and
the observer list; the rest find them. So a view reads the very object
another set, and a set in one view calls every observer of the group, its
own and other views', inside the set and in registration order — the other
views' functions run in their own realms inside the setter's entry. No Rust
code knows the API. The store goes with the group's last worker realm, as
native's shared context goes with the group's last view and web-core's
group worker with its last card.

MTS `lynx.getJSContext()` and this BTS Context are stable
`CrossThreadContext extends EventTarget` instances returned directly by
`createCrossThreadContext`. `dispatchEvent({type, data})` validates the string
type and data property, captures the public envelope, sends to the peer and
returns `0`; Context `postMessage(value)` sends a message event. Listeners
receive the original null/undefined data and an undefined receiver, and ignore
DOM listener options. The shared `bobcat:event-target` EventTarget every
Context, `Worker` and engine target extends follows the DOM's inner-invoke
rule: a listener that throws is reported and the walk continues with the next —
in the MTS realm through `lynx.reportError` and the host's `reportScriptError`,
as a nonfatal `ScriptReported` from `ScriptSource::Main`; in a worker realm
through the worker global's `reportError`, reaching the parent `Worker`'s
`error` event and a nonfatal `WorkerThrew`. The BTS reports an animation-frame,
`queueMicrotask` or `lynx.fetchBundle` callback that throws the same way, as an
uncaught exception of its realm. Only its disposal hook's throw and a misused
`SelectorQuery` go through `lynx.reportError`, as a `ScriptReported` from
the BTS's `ScriptSource`; the hook's has to, because it must reach the host
before the `disposed` reply lets MTS terminate the Worker. Origins identify
the sending CoreContext or JSContext. MTS
queues payload references until the Worker is connected; Worker postMessage
takes the structured-clone snapshot above for early and connected sends alike,
and `toJSON` is never consulted. Do not add a custom codec or a deep clone on
top of that transport; a value it refuses throws at the call. A worker's own
task queues what is posted until its bootstrap has evaluated, and BTS JS waits
on the application import before delivering later messages, so application
listeners exist before first delivery. Raw XML adapters supply the optional
entry; a compiled bundle's manifest paths and string custom sections are each
registered with the host as an ES module *under* the container URL — the
page's input URL — and the BTS boot script hands that URL alone to the modules
layer, which hangs every `requireModule` of a bundle path under it before
loading it synchronously. Each view costs one additional realm on the group's
worker runtime, and so does each `<frame>` card it shows.

MTS boot does not await BTS: `ScriptFinished` means MTS boot finished — the
entry module evaluated, its top-level await settled, and its first flush
committed. The BTS Worker's state is no part of it, so a BTS entry whose
top-level await never settles does not keep the view from becoming ready. A BTS
entry that throws is reported like any worker script: `reportError` in the
worker realm surfaces it at the `Worker`'s `error` event and as a nonfatal
`WorkerThrew` from the BTS's `ScriptSource`; the BTS keeps running and still
takes messages, and no BTS failure ends the view. MTS keeps its Worker
reference after that Worker ends; a post to an ended Worker is dropped by the
host, and the pre-connection FIFO holds only what the MTS entry sends before
boot constructs the Worker.

BTS also exposes stable `getApp()` and `getNativeApp()` objects. The current
app hooks receive `OnLifecycleEvent`, `publishEvent`, `publicComponentEvent`
and `callDestroyLifetimeFun`; the native app's `callLepusMethod` invokes a
named MTS global function and asynchronously returns its resolved result to an
optional callback. String `__AddEvent` handlers publish snapshots containing
target/currentTarget `dataset`, `id` and `uid`, never handles. Current PAPI
elements have no component metadata and use `publishEvent`; explicit component
calls preserve the supplied ID. An explicit JS engine `__DestroyLifetime` event
starts the same JS disposal Promise used by MTS teardown, terminating BTS only
after its acknowledgement. Rust starts and awaits MTS disposal through the
existing ESM evaluator and routes ordinary Worker events while it waits; it
neither identifies BTS nor calls its hook.
