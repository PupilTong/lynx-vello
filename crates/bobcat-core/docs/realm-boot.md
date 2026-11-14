# bobcat-core: Realm, document and boot

Part of the agent guide for `crates/bobcat-core`; the index
[crates/bobcat-core/AGENTS.md](../AGENTS.md) lists the other topics, and the
root [AGENTS.md](../../../AGENTS.md) holds the mission, the standards and
dependency policies, the workspace map, the toolchain and testing. Paths in
backticks are relative to the repository root.

The crate-private `quickjs::ScriptEngine` is the whole script surface: it
installs named host callbacks, registers named preloaded ESM bytecode, evaluates
a module through its TLA completion promise, calls an export the realm
published back, and provides the GC seam. Created on the engine-owned Lynx main
thread and never leaving it, nothing about it is `Send`. Values crossing it are
`quickjs-rust-bridge`'s `HostValue`/`HostArgument` — primitives plus opaque
structured clones, so realm values and DOM handles never cross as themselves.
The private `MainThreadRuntime` owns the realm integration and, through it, the
document. `LynxDocument`, `Viewport`, `DocumentIngredients`, `DocumentSlot`,
`new_document`, `MainThreadRuntime`, the view's link (`ToMain`, `ViewNotice`,
`Published`) and the concrete QuickJS adapter are crate-private; `Painter` is
public, and `LynxDocument` is what an embedder cannot name.

**The realm creates its own document, out of a configuration it holds.** The
Rust-generated boot module is written with the view's five page switches as
boolean literals (`const config = { defaultDisplayLinear: true, … };` — facts
Rust owns, passed as primitives, with no JSON round trip), and its first
statement is `export const document = new Document(config);`;
`bobcat:element`'s `Document` constructor calls
`createDocument(defaultDisplayLinear, defaultOverflowVisible,
enableCssSelector, enableJSDataProcessor, enableCssInheritance)`, which
reads five `HostValue::Boolean` arguments and builds the document from them plus the
`DocumentIngredients` that never reach the realm: the create-time viewport, the
validated text context and the group's style pool. It never waits: **the view's
author sheets are not mounted here** but by the first `__FlushElementTree`
(below). The construction runs under a catch, so a panic in it fails the
boot naming the phase that panicked, where any other host member's panic ends
the view as `Panicked`. A missing or non-boolean switch and
a second construction both throw, which fails the boot; the second one is
refused whichever module asks, because the ingredients are spent. The realm
holds what it built in the private `DocumentSlot` every tree member borrows; that `Rc<RefCell<…>>` exists only so same-thread native QuickJS
callbacks can reach the owner, not as a cross-thread sharing mechanism.

**The document lives exactly as long as the realm.** The boot module's exported
binding holds it from that first statement on, and nothing in the realm
releases it: no release member, no `FinalizationRegistry` over the `Document`,
and no "no document" answer a host member can give. Release is the view's task
ending: dropping the `LynxView` closes its command channel, the task returns,
and `MainThreadRuntime`'s fields drop in declaration order — the `ScriptEngine`
first, carrying the context's `Rc`, freeing the realm with the host functions
and their clones of the slot, then the runtime's own `slot` handle, which is
when the `LynxDocument` drops. That field order is the whole mechanism; there
is no `Drop` impl behind it. Every tree and attribute member therefore takes
the document unconditionally, and the one refusal left here is a second
`createDocument`. **There is no window in which a view has a task but no
document**: opening the realm is the first job of the view, queued before any
of its tasks is spawned and before anything has been fetched, and the boot
module's first statement is what creates the document — so a command that
arrived earlier is a job queued behind that one. Nothing is buffered, replayed
or dropped for want of a document.

**The first flush settles the listed sheets, then waits for the binding.**
Before anything is styled, `DocumentSlot::flush` takes the listed sheets off
the slot one at a time, in listed order, parks the job on each answer that has
not arrived — `JsThread::wait`, the view's token as the biased first arm — and
mounts it; an answer already in hand costs no wait. Until the slot's sheets are
gone the epilogue's `commit_if_dirty` returns without committing: the epilogue
runs after every entry, so parking there would stop the group on any of them,
and a commit without the sheets would publish an unstyled frame. Nothing is
lost, because boot's own flush is what commits the first frame. The sheets come
before the binding wait, so their IO overlaps the painter's construction and
the frame held for the binding already carries them. **The only two things
boot waits on are both inside that flush: the listed sheets, then the
binding.**

**A painter binding the view is what releases its first frame.** The document
is created at the create-time viewport unless a painter has already written the
seat's metrics watch, and every epilogue adopts whatever that watch holds. A
commit made before the first write is *held* rather than published — a painter
composes at its own size and cannot tell that the frame it adopted predates the
metrics it just named — and `__FlushElementTree` commits, holds, and then parks
the job it runs in on that watch, the way `adoptStyleSheet` parks on a
response, with the view's token as the biased first arm. Waking bound, it
adopts the painter's metrics and, if they moved the viewport, discards the held
frame and commits again; either way what goes out is at the painter's size.
Only the first binding is waited for: `detach` leaves the last metrics behind,
so a view moved to the background never parks its group again. Boot's last act
is a flush, so a view no painter ever binds publishes no frame, reports no
`ScriptFinished` and never becomes ready. One task of the view, `consume_metrics`,
settles the page once per change, which is what commits a resize with no
JavaScript behind it.

Main opens the realm and evaluates `bobcat:boot` as the view's first job,
before anything has been fetched: its first statement creates the document,
and it then imports the entry by the URL `create_lynx_view` resolved against
`ViewSources::base_url`, the same URL the fetcher was asked for. Nothing
parks for the entry's answer: it is a task of the view that enters the realm
when it arrives, queued behind `open_realm`. The
entry's task (`load_entry`) completes that module from the pre-issued answer,
exactly as the fetcher answered it and with the fetcher's response URL as its
`import.meta.url`, so boot's remainder (the BTS Worker, the render and the
flush) runs in the job that completes it; the entry's own request never reaches
the fetcher. The listed sheets are mounted by that flush, in listed order, as
above. Success is `ScriptFinished`; a font, realm or boot failure is
`StartupFailed`. An entry that cannot be loaded is the fetcher's own error,
because `load_entry` reads the answer before any of the entry runs and then
does not complete the module; an answer that is not a script, or a script
whose response URL is not an absolute URL, is a `LynxViewError::Script`
naming the URL. `__Card__` — the base every `new Worker` URL is joined to,
boot's `bobcat:bts` included — is the view's `ViewSources::card_url` when it
names one, and otherwise that response URL: a container page names its
container URL `U` there, because its entry is `<U>/root.js`, one of the
container's sections, while a raw entry or an XML page's entry is the page
itself. A relative `new Worker` URL in a container page therefore joins `U`,
not the entry. A sheet that cannot be loaded, or that
the fetcher answered with something else, is `LynxViewError::Script`, because
boot's `__FlushElementTree` is what threw, naming the sheet and the reason.
**An entry that throws does not fail the boot**: the entry is app code, and
boot wraps its `import` in a `try`/`catch` that raises what it caught again as
a rejection nothing handles. The checkpoint of the entry into the realm that
ran the catch reports it — `ScriptRunError` from `load_entry`, `load_module`
or `settle_future`, and `TimerFailed` or `ListenerFailed` where the entry's
top-level `await` resumed in a timer or a worker event — and boot goes on to
connect the BTS, render and flush, so `ScriptFinished` still follows its
flush, as a native MTS still renders a page whose script threw. What fails
boot's own evaluation is the engine's code alone: page data that is not JSON,
the document's construction, connecting the BTS, and the flush. A failure
stays the failing view's and is reported once: an entry reports the one
rejection its checkpoint returned and drops the rest it left in the
promise-job queue the group's realms share, a checkpoint error beside a
failure it is already reporting included, so none of them reaches the next
view or the failing realm's own next entry. Queued *jobs* still run, and the
next checkpoint finishes them. A checkpoint
drains that queue until it is empty, as a browser's microtask checkpoint does:
there is no per-checkpoint job budget and no incomplete checkpoint for a later
entry to resume. Because the queue is the runtime's, a view has to notice a
sibling's entry into JavaScript: `ScriptEngine::checkpoint` bumps a
runtime-wide generation on a `watch<u64>`, every view has a task following it,
and a page whose import finished inside a sibling's checkpoint settles what its
own realm owes. Comparing that generation against the one recorded at the end
of the page's own last entry keeps a page's own bumps from waking it.

**The code cache** (`code_cache.rs`; `docs/resource-cache-design.md`, "PR2").
Every text a host answers a realm with — a module completion
(`ScriptEngine::complete_module`, which MTS, BTS, Workers, a worker's root and
a `ShadowRealm` child all go through) and a synchronous load
(`register_synchronous_loader`) — is compiled through `RealmCodeCache`, which
looks the bytecode up in `vant::artifact::Artifacts::global()` at that moment,
in memory and synchronously, and otherwise asks the bridge to keep the
compile's. The lookup is the engine's and not the fetcher's because only the
engine knows what the compile bakes in. The rules:

- The key is the `SourceDigest` of exactly the text handed over, and a
  `Variant` of what else the bytes depend on: for a module its name and
  whether the runtime strips source text; for a CommonJS wrapper its response
  URL, its parameter list and the strip flag. A module's response URL
  (`import.meta.url`) is set after the read and is no part of the key. The
  `EngineId` is `bobcat_core::script_engine_id()`, the bridge's
  `bytecode_engine_id()`.
- A text under `MIN_CACHED_SOURCE_BYTES` (1 KiB) is compiled as without a
  cache and never hashed, as is every text while the tier's budget is 0. JSON
  compiles nothing. Built-in modules registered on the runtime are not host
  answers and take no part.
- What the bridge kept is filed at the end of the operation that compiled it:
  `ScriptEngine::checkpoint` and the job drain call `file_compiled`, which
  empties `Context::take_compiled` into the tier, where the resource system's
  flush finds it.
- The bridge names a compile by what it was compiled under, and a module is
  compiled when first linked, possibly operations after its text arrived. The
  realm therefore keeps, per name, the digest of the text it handed over, and
  files a compile only while every text handed over under that name was the
  same one; two different texts under one name file nothing in that realm
  (`Expected::Ambiguous`). Bytecode is never filed under a text it might not be
  the compile of.
- A hit changes nothing observable: exports, `import.meta.url`, stack frames
  and `Function.prototype.toString` are the text's. Bytecode that cannot be
  read falls back to the text inside the bridge and the working compile
  replaces it.

A `.web.bundle`'s `lepusCode.root` or raw XML main body becomes a real ESM at
its resolved entry URL — a container's at `<U>/root.js` — and it is
`bobcat-source` that gives it named imports
from both built-ins (`MTS_CHUNK_PREAMBLE`, on the body's first line) when it
registers it. Core completes the entry with what the fetcher answered and adds
nothing, so an entry that is not a card's body imports what it uses itself,
and one naming a binding it did not import fails with a `ReferenceError`, the
entry's own failure. The
`bobcat:boot` ESM imports its lifecycle helpers from `bobcat:runtime`,
`Document` and `__FlushElementTree` from `bobcat:element`, and
`bobcat:timers` for its effect. The runtime parses the view's `init_data` and `global_props`
JSON in MTS; missing values become `{}` and malformed inputs fail boot. Boot
creates its document, initializes MTS inputs, retains the host render argument,
then awaits the entry. The task that completes the entry calls
`__BobcatInitEntry` with the card URL — `ViewSources::card_url`, or the
entry's response URL for a view that named none — before it completes the
module, so `__Card__` is that URL before the entry's body runs, and a
`new Worker` URL resolves against it: the realm hands it to
`createWorker(url, name, baseUrl)`, and Rust joins the two by URL rules and
does not keep `__Card__`. Boot then processes that argument and posts
the result plus host props as the first BTS Worker message,
before rendering. The BTS bootstrap returns after installing a JS receiver, and
that message initializes its inputs before importing the entry. Later internal
messages wait on the import Promise and are delivered in order once it settles,
success or failure.

Lifecycle hooks and engine listeners run synchronously, with no intervening
Promise-job checkpoint. Boot awaits a `Promise.resolve().then` flush after
rendering; that flush's commit completes MTS boot, the whole of public
readiness, and BTS acknowledges nothing. Boot reads
`enableJSDataProcessor` off the configuration written into it, and the screen
`SystemInfo` reports is written in as three number literals from
`ViewSources::screen` — what the embedder measured: web-core's
`devicePixelRatio` and available screen size in a browser, the window's monitor
natively — or, for a host with no screen, the `ScreenMetrics::for_viewport` of
its capture size that host named explicitly. It is read once and never updated,
so a painter binding at other metrics leaves it alone. `ViewSources.initial_processor` is a plain `String`,
handed to JS by the one-shot startup-data binding without JSON serialization or
source interpolation. JS merges those three numbers over its own runtime
constants into SystemInfo; BTS builds its own out of that `SystemInfo`, which
the MTS realm posts to it in the `initialize` message, so both realms report
the numbers the MTS literals read as. A card's entry receives runtime
bindings through the ESM imports `bobcat-source` prepended; global props
updates replace the live module binding, and there is no native evaluator or
separate Script lexical environment.

Runtime JS reads ReactLynx's hooks directly from `globalThis`, while runtime
and PAPI identifiers remain module imports; a named Lepus chunk is a plain
script resource `ContainerSources` registers verbatim at
`<container>/<encoded name>.js`, which `__LoadLepusChunk` builds the URL of
from `__Card__`, loads through the same synchronous host loader a `require`
uses, and runs — again on every call, as native does — as a function body
whose parameters are the bindings `MTS_CHUNK_PREAMBLE` gives the entry. Named
calls and replies belong to the two JS Worker message handlers; Rust
transports opaque messages and performs no Lepus-specific dispatch or reply
flush. Boot's deferred flush
uses ordinary Promise scheduling and the existing outer checkpoint, with its
rejection attribution and generation notification. See
`docs/mts-execution-runtime.md` for the boot and chunk execution boundaries.

`LynxView::{update_data, reset_data, update_global_props, reload}` use the
existing ordered command/Worker links. Embedders serialize data and global
event argument lists into `String`; core passes them unchanged to JS, which
parses them and builds Worker messages. Update/reset/reload take a separate
processor-name `String`, an empty name selecting the default processor. All are
accepted once MTS boot has finished and otherwise return
`EngineError::NotReady`, like global events; the BTS Worker still loading is no
reason to refuse one. Initial data and props come from `ViewSources`; there is
no early props cache or initial-render update gate. MTS runs its own hook at
once and forwards the update to BTS with `Worker.postMessage`; the BTS runtime
queues that message behind its entry import and delivers it in order once the
import settles. Rust never sends page data to BTS — MTS `postMessage` is the
only path. A reload retains the realms and entry; the framework recreates
component state. See `docs/data-lifecycle-runtime.md` for processor selection,
snapshots, readiness, engine-event precedence and the BTS reload callback
boundary, and `docs/events-diagnostics-runtime.md` for native Context behavior,
the BTS GlobalEventEmitter and `LynxView::send_global_event`.

`LynxView::pump` records readiness before returning `ScriptFinished`, exposed
by `is_ready()`. Global events require that observed MTS boot and otherwise
return `EngineError::NotReady`; rejected events are never queued or replayed,
accepted ones retain host FIFO order. Internal pre-connection MTS messages
still wait for Worker construction. `ScriptReported` and `ConsoleMessage` are
nonfatal host notices that every realm — MTS, BTS or a `Worker` — sends to
the view's host itself through its `HostOutbox`, each carrying the realm's
`ScriptSource`; the BTS's do not pass through MTS as Worker messages. One
realm's arrive in the order it made them, and they have no order relative to
another realm's or to the worker events. `lynx.getEngine()` returns one
stable, realm-local `EventTarget` whose listeners never cross the host
boundary. Render, update,
component removal and global-prop events carry argument arrays, taking
precedence over legacy global hooks; listeners receive the engine as `this`,
with no `origin` field. The MTS `getCoreContext` and `getNative` sinks retain
and deliver nothing, and the module does not invent the background-only
`lynxCoreInject` realm.
