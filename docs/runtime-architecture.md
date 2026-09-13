# Runtime architecture

Bobcat exposes two runtime objects to an embedder: `bobcat_core::LynxView`,
which is a page, and `bobcat_core::Painter`, which is where a page's pixels
go. They are built separately on the same embedder thread and joined at
runtime by `Painter::attach`. The document, Element-PAPI tree, script realm,
and the commit/publish protocol are implementation state. An embedder supplies
only capabilities and OS facts:

- a `ViewSources` — page config, owned font bytes, an optional default font
  family, author stylesheet URLs in cascade order, and the one entry MTS
  module URL, plus optional `init_data` and `global_props` JSON text — and,
  as a separate argument, the builder of the view's
  `ResourceFetcher`, which is also its `FrameImages` and owns every byte and
  pixel the view ever loads (`crates/bobcat-resources` is the reference
  implementation all shipped embedders use);
- an `EventRequester`, the one wakeup the engine has for the embedder's
  thread. It is a platform type, not a trait object — `LynxGroup::new` is
  generic over it and the Lynx main thread holds it, so the wake is a direct
  call. One serves a whole group: its views paint on the thread that created
  it, and so wake one event loop. `NoWakeup` is the implementation for a host
  with no event loop to wake;
- a `DrawTarget`, at `Painter::new`: `DrawTarget::window(...)` over anything
  convertible into a `WindowTarget` — a `'static` surface target, so a shared
  window handle rather than a borrow — or `DrawTarget::Offscreen` for a
  windowless GPU target. A painter names it once and keeps it for its whole
  life; a view never has one at all;
- viewport/device metrics and normalized input events, both to the painter,
  which is the side that owns the surface;
- platform initialization, worker bootstrap, and file/network IO.

No clock is among them. The animation timeline is engine-owned, and so are the
realms' own timer deadlines: nothing about waiting is a host protocol.

The source tree mirrors the runtime's owners:

```text
crates/bobcat-core/src/
  view/lib.rs          LynxGroup, LynxView, ViewSources, DrawTarget, and the
                       vocabulary of the one thread boundary they cross
  link.rs              one view's link: ToMain, ViewNotice, Published, and
                       the outbox the main side publishes on
  lifetime.rs          what a view and a worker are made of alike: their tasks,
                       the token that ends them, and the owner's wait
  clock.rs             the one clock realm timers are armed against, and
                       `sleep_until`, which picks a waiter by target
  alarm.rs             wasm32 only: the bobcat-alarm thread that serves those
                       waits where tokio's timer cannot
  paint/lib.rs         Painter: attach/detach, the frame clock, routing,
                       composition, and the adopted snapshot
  paint/images.rs      one commit's resolved pixels, in draw order
  paint/gesture.rs     input arbitration
  paint/graphics.rs    window GPU state
  main/lib.rs          bobcat-main: the thread, the group task, and the style
                       pool and script runtime a group shares
  main/page.rs         one view's page: its tasks — boot, commands, module
                       loads, worker events, timers, checkpoints — and the one
                       boundary they enter JavaScript through
  main/quickjs.rs      owner-thread-bound QuickJS adapter
  main/runtime/lib.rs  realm/DOM integration, the document slot, the one member
                       that creates it and the tree members that drive it
  main/workers.rs      main-thread Worker construction and commands
  main/tree/lib.rs     Lynx document and UA component policy
  background/lib.rs    the group's worker realms: keys, commands, and the
                       one sender everything that names a worker holds
  background/thread.rs bobcat-workers and its second QuickJS runtime
  background/scope.rs  what one worker realm is made of
  test_support.rs      the in-crate doubles a test drives a view with
  threads.rs           how a thread of this engine is joined, and how it
                       reports having trapped
```

Shared viewport and source vocabulary stays in `view` beside the public
handles; the link one view speaks over is `link.rs`, owned by neither side;
a stateful type whose owner is fixed lives under `paint` or `main`.
Construction sends `ViewSources` whole to the view's task on `bobcat-main`,
since nothing in it belongs on the embedder's thread. The task stages the
document inputs, and what the source specifiers fetch, as the ingredients the
realm's own `Document` will be built from, and hands the page data to the
realm.

`ViewSources::init_data` and `global_props` are optional JSON text, and Rust
never reads it. `MainThreadRuntime::new` puts each behind a
`bobcat-internal:host` member of its own, `initData` and `globalProps`, which
hands the string over once, as a plain string. `bobcat:runtime` calls both as
it evaluates and parses them: the global props become `__globalProps` and
`lynx.__globalProps`, and the init data becomes `__BobcatInitData`, which boot
hands to `processData`. A value that was not given arrives as `undefined` and
is `{}` there, as in web-core. Text that is not JSON fails boot with
`StartupFailed`, naming the input, before the entry runs. The background
thread does not receive either value yet.

Main asks for loads through the view's own `ViewNotice` channel, and
`LynxView::pump` is what hands each ask to the host's `ResourceFetcher`.
Fetched source bytes cross to `main`; the fetcher, caches, and decoded images
never do.

The dependency graph is:

`bobcat-cli` is one native embedder crate. Its independent `cli` and `server`
features gate the two modules and their optional dependencies; both are enabled
by default. The `bobcat` and `bobcat-server` binaries require `cli` and `server`
respectively. The product labels below name those two features of the same
crate, not separate crates. Both use the complete source/resource APIs.

```text
bobcat-cli    ─┬──▶ bobcat-source
               └──▶ bobcat-resources ─┐
bobcat-server ─┬──▶ bobcat-source     ├──▶ bobcat-core ──▶ dom ─┬─▶ hughie
               └──▶ bobcat-resources ─┤          │              ├─▶ vendor/stylo
bobcat-wasm ──┬───▶ bobcat-resources ─┘          │              └─▶ vello/wgpu
              └───▶ bobcat-source
bobcat-source ────────────────────────────────────┘
                                                  └──▶ quickjs-rust-bridge

QuickJS preloaded ESM graph — bobcat-main's runtime
  bobcat:boot
    ├──▶ bobcat:element (Document class + flush binding)
    ├──▶ bobcat:timers (timer-global installation)
    ├──  export const document = new Document()   the realm's first statement
    │      └──▶ bobcat-internal:host.createDocument
    │            └──▶ DocumentIngredients ──▶ private dom::Document<()> tree
    └──▶ await import(resolved entry MTS URL)
          ├──▶ bobcat:runtime (packages/bobcat-element/src/main-thread-runtime.ts)
          │     ├── named compatibility exports + engine EventTarget
          │     ├──▶ bobcat:cross-thread-context (MTS getJSContext)
          │     └──▶ bobcat:event-target (packages/bobcat-element/src/event-target.ts)
          ├──▶ bobcat-internal (explicit import; Worker class in worker.ts)
          │     ├──▶ bobcat:event-target
          │     └──▶ bobcat-internal:host (createWorker, sendWorkerMessage, terminateWorker)
          └──▶ bobcat:element (packages/bobcat-element/src/element-papi.ts)
                └──▶ bobcat-internal:host (native named function exports)
                      └──▶ the document created above

QuickJS preloaded ESM graph — the group's worker runtime, on bobcat-workers
  bobcat:worker-boot (one per live worker, evaluated, never registered)
    ├──▶ bobcat:worker (packages/bobcat-element/src/worker-runtime.ts)
    │     ├── the global scope: self, postMessage, close, name, onmessage
    │     ├──▶ bobcat:event-target
    │     └──▶ bobcat-internal:worker (postWorkerMessage, closeWorker)
    ├──▶ bobcat:timers ──▶ bobcat-internal:host (setTimer, clearTimer only)
    └── the worker's entry source
          └── bobcat:bts (bootstrap)
                ├──▶ bobcat:bts-runtime exports lynx
                │     └──▶ bobcat:cross-thread-context ──▶ bobcat:event-target
                └──▶ await import(BTS entry) when configured
                      Application module loading: deferred
  No bobcat:element and no bobcat:runtime here: a worker has no document to
  reach and no page to be the main thread of, so reaching for either fails to
  resolve rather than failing late.

bobcat-cli ──▶ bobcat-source + winit
bobcat-wasm ──▶ bobcat-source + wasm-bindgen + wasm_thread
bobcat-server ──▶ axum multipart + reqwest + bobcat-source + BMP V4 encoding
```

## Animation timeline

The engine owns it. `crate::paint::FrameClock` is private to `bobcat-core`,
concrete, and constructed with the `Painter`: there is no trait, no type
parameter, and no constructor that takes one. It reads the platform's monotonic
clock through `crate::clock::ClockInstant` — `std::time::Instant` natively and
`web_time::Instant` on Wasm, the same split `quickjs-rust-bridge` already uses.
Its epoch is a *view's* construction rather than the painter's: `attach`
rebases it onto the view's timeline epoch, so a painter that changes views does
not restart the new one's animations at whatever the painter's own age happens
to be, and a painter attached to a fresh view reads exactly what it would have
before. A host that arranges nothing gets running animations, because arranging
nothing is the only option.

No host has a better reading to offer. Presentation runs on
`PresentMode::AutoVsync`, so the swap chain paces frames, and the engine
samples *after* the acquire that waits on it (below). A browser's
`requestAnimationFrame` timestamp is not the improvement it looks like: it is
taken on the page's main thread, before the Render Worker is woken, and on a
different time origin than the Worker's own `performance.now()`.

**The frame's one reading.** `draw` and `tick` each call
`FrameClock::now_seconds` exactly once and pass that `f64` to everything the
frame resolves — `service_gesture_clock` for armed `longpress` deadlines on
the painting side, and the `BeginFrame` command that carries the same
reading to the main thread's `advance_animations` — so a gesture and an
animation in the same frame cannot disagree about when the frame is. Input
arrival is the one other reading, taken in `dispatch_input` at the moment the
event arrives.

**Where the reading is taken.** A window frame is `WindowGraphics::acquire`,
then `render_to_target`, then `present`. Acquiring first is deliberate: under
`AutoVsync` the swap chain hands over an image only once one is free, so
`acquire` is the call that blocks, and every image in flight is another display
refresh between that wait and scan-out. Sampling the clock after it puts the
whole frame on the near side of the pipeline — what is sampled is the frame
being produced for the next refresh, not one produced a pipeline-depth earlier.
The wait is the embedder's own turn spent waiting for its display, and it
touches no document and takes no lock. An offscreen `tick` has no swap chain
and samples immediately.

`dom` itself reads no clock: `now` is a parameter to
`Document::advance_animations`. That is what lets the painting side decide
the instant, not the document.

Advancing an animation runs where the document is — the Lynx main thread,
its only home. The painter sends one `BeginFrame { now, seq }` command per
frame while the latest committed frame reports an active animation, and the
view's command consumer advances the timeline — a Stylo animation-only
traversal of just the animating elements, no JavaScript involved — and
commits what changed. The `seq` is what an offscreen `tick` waits on: the
epilogue publishes the newest serviced sequence number on the view's watch
after the commit it implies, so a host blocked on that number is woken by the
frame rather than by the acknowledgement. The published frame's
`animations_active` flag is what keeps the loop sustained: `owes_frame` keeps
answering yes, the embedder keeps taking a turn per display frame, and each
one sends `BeginFrame` until a commit reports the timeline idle. Starting and cancelling animations belong to the style
flush the main thread already runs at `__FlushElementTree`.

`bobcat-core` deliberately does not re-export `dom`. The lower-layer crates
remain independently usable libraries, but an application embedding Bobcat
cannot reach them through a running view.

## Startup boundary

Source/container IO is embedder work. `bobcat-core` does not fetch, decode, or
interpret `.web.bundle` containers or Lynx XML envelopes and has no public
decoder/parser types for either format.

For the native file and HTTP products, `bobcat-source` owns the shared
container-to-runtime mapping while each embedder owns resources, transport,
and execution:

1. `bobcat-cli` reads one local input, while `bobcat-server` reads one
   `file://`, `http://`, or `https://` input for each accepted capture job;
2. `bobcat-source::PageSource` content-sniffs Lynx XML, web bundles and
   source-based native bundles, uses the shared parsers, lowers bundle
   `StyleInfo` directly to a `PreparsedStyleSheet`, and produces the
   corresponding `PageConfig`, source URLs, and payloads;
3. the embedder registers `lepusCode.root` or the XML main-thread body as the
   entry MTS module in its `bobcat-resources` instance, alongside the bundle
   stylesheet on the pre-parsed arm or an XML `<style>` body on the CSS-text
   arm; that resource system also owns every later file/network fetch, cache,
   and image decode;
4. the embedder starts a `LynxGroup` for that page and calls
   `create_lynx_view` with the `bobcat-resources` per-view builder and the
   resulting `ViewSources`, then builds a `Painter` over its draw target and
   attaches it to that view.

All embedders use the complete source crate without feature flags. Binary
page inputs require a `root` module; native bytecode remains unsupported.

`bobcat-server` keeps HTTP handling concurrent but sends accepted captures
through a bounded FIFO to one dedicated capture thread. For each job that
thread is the embedder owner: it creates a fresh `LynxGroup` for that job,
constructs its non-`Send` `LynxView`, builds an 800×600 DPR-1
`DrawTarget::Offscreen` `Painter` beside it and attaches the two, owns all GPU
work, settles the page on a plain frame interval — `view.pump()` and then
`painter.tick(false)` per step — captures tightly packed RGBA8, and destroys
both.
The group owns the Lynx main thread, QuickJS runtime, and Stylo pool, all
released with that job's view; no runtime is shared across capture jobs and
the server adds no separate rendering owner. The HTTP side then encodes the
frame as an uncompressed BMP on Tokio's blocking pool after compositing over
white, so CPU encoding neither retains the view nor occupies the GPU lane.
A worker panic makes health fail and initiates server shutdown; queue saturation fails
admission rather than creating unbounded GPU work. This is a trusted-page
embedder: the public core deliberately exposes no QuickJS interrupt, and
`timeoutMs` cannot preempt synchronous script execution, a blocking GPU call
on the capture/embedder thread, or synchronous view teardown while it joins
the Lynx main thread.

The browser reference embedder uses the shared `register_lynx_xml_response` adapter inside
its Render Worker after fetching one XML URL. Native and browser XML adapters
register the optional background script and pass its URL as
`ViewSources.background_entry`; `bobcat:bts` imports that entry after
initializing `lynx.getCoreContext()`. Application module loading through
ResourceFetcher is deferred, so an entry not preloaded in QuickJS reports an
import error. Compiled bundle manifests also need the Lynx Core module/init shell.

`LynxGroup::new` awaits the shared script runtime and style pool.
`create_lynx_view` validates metrics, sends the view's half of its link to the
group's thread, and builds the host's fetcher on the calling thread. It is
synchronous — nothing it builds can block — and returns a loading view. Only
metrics and attachment failures are returned by construction.

The view's own task on `bobcat-main` runs boot as a straight-line async
function. Fonts and the default family come first, validated against a
`dom::TextContext` of their own: they are a text context's business, no
document exists yet, and an unknown default family therefore stays a
zero-fetch `EngineError::UnknownFontFamily`. Then each author stylesheet in
cascade order, then the entry. Each ask leaves as a `ViewNotice::RequestSource`
carrying the request and the right to answer it; `LynxView::pump` is what hands
that to `ResourceFetcher::request_source`.
The fetcher resolves the URL, fetches bytes and validates UTF-8, or supplies a
pre-parsed stylesheet. Completion consumes the handle and answers the one-shot
minted with the request, which wakes whichever task was awaiting that source —
a stylesheet or entry on `bobcat-main`, a worker script on `bobcat-workers` —
without a turn anywhere else. The handle contains that sender and a clone of the view's
`CancellationToken`: no erased callback, retained resource Future, `SourceLoads`
or `EventWaker` is needed. The
fetcher itself is owned by value and needs neither `Send`, `Sync` nor `'static`.
The reference fetcher queues a concrete source job on its native pool, or starts
a browser task on Wasm. A view's task awaits no IO on any other view's behalf,
so a sibling can boot or handle events while this view loads.

Sheets and the entry are **staged, not mounted**: there is no document to mount
them on until the realm's boot module creates one. What the task assembles is
`DocumentIngredients` — the viewport, the `PageConfig`, the validated text
context, the fetched sheets in cascade order, the group's style pool, and any
image reports that arrived first — and `MainThreadRuntime::new` opens the realm
holding them in its `DocumentSlot`.

The response carries a loaded source or error. Main owns the boot outcome:
`ScriptFinished` reports success; `StartupFailed(LynxViewError)` reports resource,
encoding, font, realm or boot failure exactly once through `LynxView::pump`.
A failed view asks its host for nothing more, sources and images alike.
Its resolved entry URL is the module specifier.
`ScriptRunError` reports fatal runtime failure; listener and timer failures stay
non-fatal. Every main notification requests a host turn through `EventRequester`.

Dropping a loading view cancels its `CancellationToken`, detaches its image
inbox, and then closes its command channel; boot never enters QuickJS after the
cancel. The token is cancelled synchronously on the embedder's thread, so a host
still holding a completion reads cancellation without waiting for a turn. A burst
queued behind the release is discarded rather than applied: the view's one
command consumer reads the token itself at each wake, before applying anything,
and the release is cancelled before the command sender drops so that read
already says released. Wake order is not what does it — a command already queued
wakes that consumer ahead of the owner. A burst already inside the entry when the
cancel lands finishes. A fetcher checks the completion handle before
queued IO and after IO, skipping unnecessary decoding. IO and JavaScript already
executing may finish; cancelled completions are discarded. Dropping an unanswered
completion for a live view reports a resource failure, including when a worker
exits before answering. Other views and their group remain alive; the last
group/view handle joins the group's threads.

Source requests select an entry or stylesheet payload and carry a specifier;
the fetcher owns base URL and transport policy. That call, `request_image`,
`service_images` and the `FrameImages` supertrait are the whole protocol, and
every one of them is synchronous — core holds no resource future, and core
names none of a fetcher's own transport API. The protocol carries no
response-size limit; each fetcher owns the bound for the response it
materializes.

A view is a set of tasks on the group's `LocalSet`, one per thing it can wait
for, and tokio owns the polling, parking and waking. `serve_view` is the owner
and has exactly one wait of its own — the view's end; `boot_page` requests the
sheets in cascade order, then the entry, then opens the realm; `consume_commands`
is the one ordered consumer of the command channel; `consume_worker_events` is
the one ordered consumer of this view's workers; one `load_module` future runs
per resource load an import produced; `serve_clock` owns the realm's one pinned
sleep and watches the runtime-wide checkpoint generation.
Nothing is spawned per input: an ordered stream stays serial because one
consumer reads it with `while let Some(x) = rx.recv().await`.

Every one of them reaches the realm through `Page::enter`, the one JavaScript
execution boundary. It runs one synchronous operation under the borrows of the
shared runtime and the realm, and then the epilogue, in this order — due timers
first, because whatever just ran may have armed or cleared one and its mutation
should ride the same frame; the commit next, so the frame exists before
anything implying it; then the boot report, the `BeginFrame` acknowledgement,
the module requests that entry produced, the next timer deadline republished
only when it moved, and finally the checkpoint generation as of this entry.
`Page::settle` is the epilogue alone, for a wake that carries no operation of
its own. `Page::open_realm` is the one documented exception, because the realm
it would enter does not exist until it returns. A command opens a burst: the
rest of what is already queued goes with it, bounded by the length the count
was taken from, so a host's whole round of input is one entry, one commit and
one acknowledgement rather than one of each per command.

That checkpoint watch is a runtime-wide `u64` bumped inside
`ScriptEngine::checkpoint`. The promise-job queue belongs to the runtime rather
than to any realm, so a view whose import finished inside a *sibling's* entry
into JavaScript has to settle what its own realm owes; the checkpoint arm of
`serve_clock` is how it learns to, and comparing the generation against the one
the epilogue recorded is what keeps a page's own entries from waking it.

An end is one signal rather than a message anything has to race. A view and a
worker are both built from `lifetime.rs`'s `Lifetime`: the `JoinSet` holding
that object's tasks, the `CancellationToken` that ends them, a thread-local
latch, and the deadline and checkpoint generation that object's one
`serve_clock` task reads. `Page::end` — the command channel closing, a cancelled
load, a fatal failure, a panic in any task — sets the latch synchronously,
cancels the token, withdraws the armed deadline, and acknowledges whatever
`BeginFrame` was pending so a blocked painter is released. Every entry point
returns at once when the latch is set; the owner, whose one wait is the token
versus the next task to finish, then mirrors a cancellation that came from
another thread onto that latch, aborts and awaits every task of the view — which
is what makes it the last owner of the page — and drops the realm. Why a view
ended is recorded nowhere: what the embedder was told is whatever was reported
before the end, and a release is the token having been cancelled from outside. A
panic is the one end that still owes a report, and the payload rides the
`JoinError` the set yields.

## Public and private boundaries

The public facade is two handles. `LynxView` is the page: it owns the host's
resource system, and its `pump` is the only call that advances the resource
protocol — hand out the source asks, give the fetcher its `service_images`
moment, name the sources the last walk discovered, write the completed loads
back, and return the lifecycle events the turn produced. `Painter` is where
pixels go: input, resize, occlusion, refresh, offscreen ticks, capture, and a
`pump` that draws the frame it owes. Neither exposes a tree getter, document
getter, script-realm handle, decomposition method, or way to mount a
stylesheet or start a second entry module. `LynxView<F>` carries only the
view-owned `ResourceFetcher` type. The embedder's event-loop wakeup is a
separate group constructor generic held by `bobcat-main`, not another owner or
thread represented in either handle.

The following types are private to `bobcat-core`:

- the link — `ToMain`, `ViewNotice`, and `Published`;
- `MainThreadRuntime`, its `DocumentSlot` and `DocumentIngredients`, and its
  Element-PAPI host implementation;
- `LynxDocument`, `Viewport`, and `new_document`;
- the concrete QuickJS realm adapter.

This prevents an embedder from bypassing commit ordering, mutating the tree
beside JavaScript, reaching the main-thread document at all, submitting a
scene independently of the painter, or evaluating code directly in the view's
realm. A `Painter` is public, but everything it holds of a view is
non-owning — a `watch` receiver, and a `Weak` on the view's seat, which holds
the view's command sender and its handle on the host's resource system — so it
is a second *observer* rather than a second way to drive a view.

## The core-owned JavaScript engine

The script engine is not an injected capability. `bobcat-core` owns its
`QuickJS` runtimes and every realm on them outright, behind the crate-private
`main::quickjs::ScriptRuntime`/`ScriptEngine`, which are created on the thread
that owns them and never leave it — deliberately not `Send`, and unnameable
outside the crate.

A group has two runtimes, on two threads. `bobcat-main` carries one realm per
view: same heap, same atom table, same job queue, no value crossing between
them, and each with its own global object and native modules.
`bobcat-workers` — owned by the group, started by `LynxGroup::new` beside
`bobcat-main` and joined by the group handle's drop after it — carries the
other, with one task and one realm per live worker. It is an independent
runtime environment rather than something `bobcat-main` offloads work to:
`bobcat-main` holds one sender on it, sends three messages (start a context
with its script, post to a context, stop a context) and receives events back,
and nothing else crosses. Separating
them is the whole point of a worker: script that must not stop the thread that
owns the document. Because `QuickJS` binds a runtime to one thread, that
separation is also what makes "a worker cannot touch the document" structural
— there is no path from a worker realm to a `LynxDocument`, and no value of
either runtime can be named by the other.

The main realm can explicitly import `Worker` from `bobcat-internal`:

```js
import { Worker } from "bobcat-internal";
const worker = new Worker("./worker.js", { type: "module", name: "data" });
worker.onmessage = event => { /* event.data */ };
worker.postMessage({ command: "start" });
// worker.terminate();
```

Every construction opens its own context on the existing `bobcat-workers`
thread. The class is an `EventTarget` with `onmessage` and `onerror`; it is
neither installed on `globalThis` nor available inside a worker. Modules are
the only script kind, also when `type` is omitted; explicit `classic` is
rejected. This internal API currently retains the worker scope's JSON
transport (`JSON.stringify([message])`), not structured clone. Transfer lists,
external module fetching, credentials options, and worker-local `onerror`
remain unsupported. For example, `undefined` becomes `null`, cycles and
BigInt throw, and typed arrays do not preserve their type.

```text
main realm: new Worker(url)
  ├── WorkerStart { key, name, script: oneshot receiver,
  │                 messages: mpsc receiver, events: this view's sender }
  │        ────────────────────────────────▶ bobcat-workers: one task per worker
  └── ViewNotice::RequestSource ──▶ LynxView::pump ──▶ request_source
                                     │ SourceRequest::Worker {specifier, base_url}
                                     └── SourceCompletion answers the oneshot
                                         that already rode inside the Start
main realm: postMessage / terminate ───────────────▶ that worker's own task
main realm: Worker message/error handler ◀── WorkerEvent { key, payload }
```

The base URL is the creating view's resolved entry URL; resolution, fetching
and UTF-8 validation remain fetcher policy. Multiple worker requests are
preserved without coalescing. The `WorkerStart` is sent before the host is
asked to fetch, so messages posted during loading queue against an existing
key — the worker's task holds them until its scope exists, which is what HTML
does. The completion answers the worker's task directly: it needs no
main-thread turn and cannot be held behind a long main-thread script. A worker
told to terminate before its script arrives never boots, because that wait is
a `biased` select with the message channel first; behind that arm the same
wait watches the worker's own cancellation token, so a view released while a
script is in flight ends its workers without a message reaching any of them.

Worker keys are allocated once per group on main and never reused. A worker's
whole state is its own task; `bobcat-main` keeps nothing per worker but the
sending end of its message channel, and only while that worker runs — a worker
that closed itself or failed is forgotten where the realm learns of it, when
that event is dispatched. A released realm sends a terminate on
every sender it still holds, which is how a view stops the workers it created.
A closed channel, and the view's own cancellation token — every worker it
created holds a child of it, so cancelling the view's cancels theirs — end a
worker too, but those are the backstop, for a realm that was gone before it
could say anything, rather than the protocol. The main
realm retains Worker objects until termination, close, or load failure.
`terminate()` immediately removes the sending handle and asks that worker to
end its context between tasks, discarding whatever was queued behind it; it
does not interrupt synchronous JavaScript.
Late source results and messages cannot restart or reach a terminated context.
Releasing the view, including after failed entry boot, cancels its source work
and stops its workers — one terminate each, sent as the realm goes. The senders
they were reachable through drop behind those messages; that is the backstop,
not the protocol. Worker errors reach the
parent's `error` handler and the embedder as nonfatal `WorkerFailed`; a parent
handler that throws reports `ListenerFailed` and leaves the view serving.

Each successful MTS entry import now starts one BTS Worker named `lynx-bg`.
Boot constructs it through the same `bobcat-internal` class, using the reserved
module `bobcat:bts`. All workers use the same scope and protocol. BTS `lynx`
is an ESM export from `bobcat:bts-runtime`; neither MTS nor BTS sets
`globalThis.lynx`. The bootstrap and BTS application entry preamble both use
`import { lynx } from "bobcat:bts-runtime"`, matching MTS's named import from
`bobcat:runtime`. The application therefore imports its bindings without
creating a dependency back to the bootstrap awaiting it.
Main answers the built-in `bobcat:bts` source itself, on the one-shot that
rode to `bobcat-workers` inside the `Start`, rather than asking a host that has
no bytes for it. When `ViewSources.background_entry` is
configured, the bootstrap appends `await import(entry)`, matching MTS boot's
import structure. XML takes exactly this path; no application source is
prefetched or concatenated into the bootstrap. Without an entry, the bootstrap
initializes the Context and the app/native-app hook surfaces.

BTS application module loading through ResourceFetcher is explicitly deferred.
This change adds no module collection, loader API or realm-local source
registry. Current imports require a preloaded module; otherwise the normal
nonfatal `WorkerFailed` event reports the missing source. Context tests preload
a fixture using the existing runtime API. The runtime cost remains one worker
realm per view, with no additional OS thread or runtime.

MTS `lynx.getJSContext()` and BTS `lynx.getCoreContext()` return stable
`CrossThreadContext extends EventTarget` instances. Their native Lynx contract
requires a string type and a data property, captures the public envelope, returns `0`
for accepted peer sends, and carries a fixed CoreContext/JSContext origin.
`postMessage` sends a `message` event; null and undefined data are retained.
Listeners require a string/function, ignore DOM options and receive undefined
as their receiver. Ordinary EventTarget behavior is unchanged. See
[events and diagnostics](events-diagnostics-runtime.md) for the contract,
BTS GlobalEventEmitter, ordered host global events and nonfatal reports.

The MTS Context exists during entry evaluation. Runtime JS projects the public
Context fields before queuing; payload objects remain references until Worker
connection posts the messages in FIFO order. Worker `postMessage` performs the
JSON copy, for early and connected sends alike. The worker's task queues what
is posted until its entry has evaluated. Worker release, source cancellation
and `WorkerFailed` reporting apply to BTS too. The built-in BTS always posts a
readiness acknowledgement after its optional entry completes. MTS receives it
and calls `notifyReady()` through the native binding; MTS boot itself never
awaits BTS. `ScriptFinished` requires both MTS completion and this declaration.
A BTS startup error uses `reportStartupFailure(message)` and reports
`StartupFailed`, independently of MTS evaluation. Ordinary Worker failures and
BTS failures after readiness remain nonfatal.
`LynxView::pump` records readiness before returning `ScriptFinished`, and
`is_ready()` exposes that state. Host global events require readiness and return
`EngineError::NotReady` otherwise, without buffering them.

The BTS runtime exposes stable `lynx.getApp()` and `lynx.getNativeApp()`
objects. MTS `__OnLifecycleEvent(data)` sends the existing Context event;
the BTS listener calls the current `app.OnLifecycleEvent(data)` with the app
as receiver. Separate runtime Worker messages carry `publishEvent`,
`publicComponentEvent`, `callDestroyLifetimeFun` and `callLepusMethod`, so
these calls do not become application Context events. Context and runtime
messages share the MTS queue before Worker connection.

String `__AddEvent` handlers, including an empty string, now publish a JSON
snapshot to BTS. Target identities contain `dataset`, `id` and `uid`; no
element handle or propagation method crosses the boundary. Catch forms still
stop the MTS walk before publishing. BTS reads the app hook at each delivery,
and each publish hook retains early calls until its first installation.
Handler names remain opaque. The component call preserves its explicit
component ID, but current Element PAPI creation has no `__CreateComponent`
or component metadata, so its string handlers use `publishEvent`. An owner
unique ID is not a framework component ID. Global handler fan-out remains
pending with the existing native event path.

`lynx.getNativeApp().callLepusMethod(name, data, callback?)` passes object
arguments directly to worker-global `postMessage`; primitive arguments are
ignored. MTS reads the current `globalThis[name]`, invokes it with that global
receiver and awaits the result before posting its reply through
`Worker.postMessage`. BTS removes the callback ID on receipt and invokes
the callback in a Promise job with one result argument. Missing methods yield
undefined; null remains null. Failed calls, rejected results and JSON
serialization errors report through the existing Worker error path without
success callbacks,
including calls without a callback. An unresolved call does not block later
requests, and each reply selects its own callback.

This follows `web-worker-rpc/src/Rpc.ts`'s `await handler(...message.data)` and
callbackify's `.then(callback)`, with web-core's `Background.ts` named global
lookup. It deliberately omits native QuickContext's nested job drain: the await
continuation snapshots the result before any later nested jobs. The full upstream
RPC registry, synchronous SharedArrayBuffer path and transfer-list support are
unnecessary for this single asynchronous endpoint.

Rust never parses a runtime envelope, selects a named method or flushes a reply
queue. The existing Worker transport, realm checkpoint and QuickJS bridge are
unchanged. The current JSON value semantics are an accepted compatibility limit:
undefined object members are omitted, undefined array entries and nonfinite
numbers become null, and negative zero becomes zero. BigInts inside messages
and cyclic objects fail serialization. No custom value codec or extra deep
clone compensates for these effects; structured clone and transfers remain
outside this endpoint's scope.

An explicit JS `lynx.getEngine().dispatchEvent({type: "__DestroyLifetime"})`
forwards a Worker message to the current BTS `app.callDestroyLifetimeFun()`
hook. This is framework event delivery only: it does not terminate the Worker,
clear pending Lepus callbacks, or release Rust objects. Automatic Rust teardown
has no added JS entry point, and no dispose API is introduced.

Its script surface covers:

- register a Rust-backed named function export in a native ESM module;
- register UTF-8 source under an exact preloaded module specifier;
- start an ESM entry and retain its evaluation promise until it settles;
- take module source requests, complete them, and resume suspended imports;
- call a named export of an already-loaded source module;
- run a collection.

The callback boundary carries only `quickjs-rust-bridge`'s primitive
`HostValue`/`HostArgument`. Objects, symbols,
functions, raw VM values, and DOM handles cannot cross it. Bobcat registers its
private callbacks as named exports of the native `bobcat-internal:host` ESM —
the document member `createDocument`, the tree and attribute
members, the three event members, the two timer members, and the three worker
members — then preloads three kinds of ESM source: the core-owned
`bobcat:runtime` named compatibility exports, the embedded `bobcat:element`
named Element-PAPI exports, and the fetched entry under its resolved URL.
`bobcat:element`
imports its native operations directly; nothing is installed as
`globalThis.bobcat`. Before registering the entry, core prepends its runtime
and Element-PAPI import declarations. Event delivery travels back through the
loaded `bobcat:element` namespace's `__BobcatDispatchEvent` export.

Because `bobcat-internal:host` resolves from any module in the realm, a card
can reach `createDocument` too. Constructing a second `Document` is refused,
which fails that card's boot, and that refusal is the only one in this area:
every tree and attribute member takes the realm's document unconditionally,
because no JavaScript runs in the realm before the boot module's first
statement creates it and nothing releases it while the realm lives.

Ordinary ECMAScript `import(specifier)` loads JavaScript ESM asynchronously,
including computed specifiers, static dependencies, re-exports, cycles and
nested dynamic imports. Core normalizes absolute and relative URLs against the
importing module's response URL; bare specifiers are limited to the built-ins.
Import attributes, JSON modules, import maps and Lynx component-bundle imports
are outside this JavaScript-module path.

The bridge keeps built-in sources on the shared runtime and entry/imported
sources on each realm. A missing module creates one `SourceRequest::Module` per
normalized URL in that realm. The boundary's epilogue spawns one task per
queued request, and `LynxView::pump` forwards each through
`ResourceFetcher::request_source`. IO never blocks the script thread. The
answer resolves that task, whose completion registers the source or the cached
load error, resumes import continuations, and drains promise jobs. Repeated
imports share the same module namespace and evaluation within a realm; sibling
views can load the same URL independently, and a sibling's checkpoint is what
tells a view its own import may have finished inside it.

The QuickJS fork adds unlinked compilation and a module-load deferrer. Before
linking or evaluating an import, it walks the static graph with an attempt-local
visited set, so missing sources can suspend without partially linking a cycle.
The bridge retains the original promise continuation on its owning context and
releases it when the context is destroyed. Loaded module code is not replayed
when another dependency arrives. Fetch/encoding errors reject with `TypeError`;
parse and evaluation failures preserve their JavaScript exception. A handled
rejection leaves the realm usable.

Boot stays pending while top-level await needs resources or timers. A host
`LynxView::pump` keeps the resources moving; the timers need nothing from a
host, because the view's task waits its own realm's deadlines out.
The boot promise tracks only MTS evaluation. Its rejection sends `StartupFailed`;
`ScriptFinished` is published after it fulfills and the MTS runtime has declared
application readiness through `notifyReady()`.
Imports started after boot use the same loading path. Dropping a view cancels
its completion handles and releases its suspended continuations.

The final `bobcat:boot` module imports `lynx`, `__BobcatConnectBackground`,
`__BobcatInitData` from `bobcat:runtime` and `Document` and
`__FlushElementTree` from `bobcat:element`, and imports `bobcat:timers` for its
effect; the transformed entry itself statically imports both of the first two
built-ins. Evaluating `bobcat:runtime` is what reads and parses the page data,
so it is ready before either module's own code runs. Boot then runs:

```js
// The realm's document, created by this module's first statement and held by
// this exported binding for the realm's life. Nothing in the realm releases
// it: it goes when the realm does.
export const document = new Document();

await import(entryMtsUrl);
const { Worker } = await import("bobcat-internal");
__BobcatConnectBackground(new Worker("bobcat:bts", { name: "lynx-bg" }));
const data = typeof globalThis.processData === "function"
  ? globalThis.processData(__BobcatInitData)
  : __BobcatInitData;
if (typeof globalThis.renderPage === "function") {
  globalThis.renderPage(data);
} else {
  lynx.getEngine().dispatchEvent({ type: "__RenderPage", data });
}
__FlushElementTree();
```

The global `renderPage` function remains a compatibility path, not a boot
requirement. An entry may instead register its renderer on the stable,
realm-local EventTarget returned by `lynx.getEngine()`. Rust evaluates one boot
module; it does not issue a second native lifecycle call after evaluating the
entry.

The engine EventTarget retains JavaScript listeners and receives only the boot
fallback's `__RenderPage` delivery today. The remaining MTS `getCoreContext`
and `getNative` sinks retain and deliver nothing. They make chunks installable before
Bobcat has the corresponding runtime subsystems; they do not install runtime
bindings on `globalThis`, create a background `lynxCoreInject` realm, or hide
missing Element PAPI members such as `__AddClass`.

The host-facing boot boundary is synchronous, but the graph is fully ESM and
supports top-level await. QuickJS drains its owned pending-job queue until the
boot module's evaluation promise settles before returning. An entry whose TLA
remains pending without another queued job is rejected rather than reported as
finished; a persistent JavaScript event loop remains a later runtime feature.

The realm, its configuration, its values, and its entry points are all
private; the only script surface an embedder sees is the sanitized
`script::ScriptError` a failure is reported with. The engine sets no execution
deadline; the underlying bridge retains an opt-in timeout for its direct users
and tests.

## Document and rendering ownership

The document belongs to a *realm*, and the realm creates it. `bobcat:element`
exports `class Document`; the boot module's first statement constructs one and
exports the binding; and that constructor calls the host member
`createDocument`, which builds a `LynxDocument` out of the
`DocumentIngredients` the view's task staged before the realm opened — the
viewport, the `PageConfig`, the validated `dom::TextContext`, the author sheets
in cascade order, the group's `StylePool`, and any image reports that arrived
first. It adopts the text context, mounts the sheets in order, and replays the
reports, each phase under its own catch because the bridge erases a panic into
"the host function panicked" and this is the one member that runs the whole
document pipeline. The ingredients are spent by the first call, so a second
`Document` is refused whichever module asks for it — and a construction that
fails rejects the boot module's own `new Document()`, which fails the boot and
ends the view, so nothing asks again.

The document then lives exactly as long as the realm. The boot module's
exported binding holds the object, nothing in the realm releases it, and there
is no release member, no registry over the `Document`, and no answer a host
member gives without a document. That is deliberately not the path elements
take: cards genuinely unroot handles, and a collection every 32 removals frees
what they named.

Release is the view's task ending. Dropping the `LynxView` closes its command
channel; the task returns and drops the `MainThreadRuntime`, whose fields drop
in declaration order — the `engine` field first, which holds the context's
`Rc`, so the realm is freed with it; freeing it takes the host functions the
realm held and their clones of the
`Rc<RefCell<DocumentSlot>>` with it, and the runtime's own `slot` handle drops
after that, which is when the `LynxDocument` drops. JavaScript goes first, then the Rust
object it named; the field order and its comment are the whole mechanism, and
no `Drop` impl stands behind them. The task drops the workers it started and
the frames watch with the same return; a painter attached to the view keeps
showing the last frame it drew.

There is one window where a view has no document, and the task serves through
it: the load, before the boot module constructs its `Document`. In it, `Resize`
writes the staged viewport; `ImageEvents` are buffered and replayed once there
is a document; `BeginFrame` is still acknowledged, so an offscreen host is
never blocked by a load; `DispatchEvent` and `Refill` are dropped, because
there is no committed tree to route them against and nothing arriving now would
still be true by the time there was.

`dom::Document<T>` privately owns its style/layout state, retained commit
builder and Vello scene; that DOM-side painter is distinct from the
embedder-thread `bobcat_core::Painter` that composes a published frame.
In Bobcat the payload is `()` and the core adds
the permanent `page` root plus Lynx UA defaults from `PageConfig`.

It also defines the two components the engine owns, `raw-text` and `image`,
each in its own module (`tree::raw_text` and `tree::image`, which own the
component, its UA rules, and its tests together). Lynx writes a
text run as an attribute (`__CreateRawText(value)` sets `text` on a `raw-text`
element) while everything downstream — Parley shaping, line breaking, the
glyph painter — speaks the W3C text node, so the component observes `text` and
reflects its value into one text node, updating that node in place and
carrying none at all for an empty value. The UA sheet supplies the display
policy the reflection needs: `text` is a flex container, `wrapper` is
`display: contents`, and a `raw-text` dissolves into the `text` it is written
inside and generates no box anywhere else.

`image` is the same shape of join for pictures: Lynx names one as an attribute
(`__CreateImage` then `src`), while everything downstream speaks replaced
content, so the component reflects `src` into `Document::set_image_source` and
an empty or removed value into no source at all. Its UA rules give the tag a
border box, suppress every child, and — the one load-bearing declaration —
`contain: size`, which is the CSS spelling of Lynx's rule that an `<image>`
box is sized by its style and never by its bitmap. Native gives the tag no
platform layout node unless it carries `auto-size`, leaving starlight to
measure it as a childless leaf that is zero on every non-definite axis, and
web-core buys the same from the browser with `contain: strict` on `x-image`;
the replaced-content path would otherwise size it like an `<img>`.

```text
private Document<()>
  ├── DOM + Stylo arenas
  ├── layout/text state
  ├── ImageRegistry          (source names and load states; no pixels)
  └── private dom paint::Painter (main-thread commit builder)
        ├── retained Arc<CommittedFrame>   (paint tables + scroll-slot table +
        │                                   the split scene: per-chain fragments
        │                                   and the compose program over them;
        │                                   the publish unit)
        └── reusable walk/build scratch
```

A commit — style flush, layout, paint-order build, scene encode — runs where
the document is and ends by publishing one immutable `Arc<CommittedFrame>` on
the view's `watch<Published>`.
The painter is a compositor over the published frame: it routes input and
recognizes gestures against the frame's tables, uploads the scene, submits,
presents, and captures. The public `EventRequester` trait describes the host
wakeup — implemented by the platform and named at `LynxGroup::new`, one per
group — and it
does not expose the engine that consumes it. There is no frame-scheduling
capability: a commit records that it wants a frame and wakes the embedder,
whose next `Painter::pump` draws it — no OS frame callback is asked for or
waited on.

Images are the host's `ResourceFetcher` and nothing else; no container
sniffing, codec, cache, byte budget or eviction policy exists in `bobcat-core`
or `dom`. The paint walk names each source it meets; the view reports those
names to the host on its own turn and asks for each
(`request_image`), the fetcher answers through the view's
`ImageReports` with the intrinsic size, and the document records the load and
recommits. A frame that names a source not yet loaded paints nothing for it,
the same not-yet-loaded state a browser shows. Each `LynxView::pump` gives the
fetcher a moment of its own (`service_images`) to forward loads that completed
elsewhere, before the sources that turn discovered are named. A painter asks
the host for nothing at all; it only *reads*, through the seat it holds a
`Weak` of, which carries the view's handle on the store the view owns.
Adopting a commit is that read: the painter resolves the frame's images once
per commit through `FrameImages::read`, synchronously and off the swap-chain
window, each with the
`ImageSizeHint` of its largest draw in the frame — the size a host decodes to.
A commit whose pixels cannot be read in the same step is not adopted, because
a frame indexes its store's bitmaps by draw order and a frame over another
commit's table would draw the wrong images; that is also why a released view's
last frame stays drawable only if the painter read it while the view was still
there.
That read may block, because after a reported load it must not miss: a store
that evicted a bitmap restores it inside the call. It happens in `poll_link`,
which every painter entry point runs first and always before the drawing path
acquires a swap-chain image, so a restore cannot stall the chain under vsync. `crates/bobcat-resources`
is the reference implementation of all of this, and `LynxView::prefetch_images`
warms sources ahead of the walk that would discover them. A Lynx `<image>`
rides the same rails: its `src` binds a source, which is what asks the host
for it.

## Commit, publish, and visibility

The document has one owner for its whole life: the realm on the engine-owned
Lynx main thread, whose boot module created it out of the ingredients that
thread staged. That thread is the only committer.

A view spans two threads and one link. The embedder's own thread — whichever
one created the view's `LynxGroup` — owns the window, captures input, creates
the surface (the one call macOS allows nowhere else), owns the host's whole
resource system, and owns everything that
draws. The Lynx main thread owns the realm, and through the realm the document.

The link is three `tokio::sync` channels, none of them addressed, because
there is nobody else on the wire: the embedder's thread holds one sending end
and two receiving ends, and the task serving that view holds the others. A
sibling's traffic is not on this path at all, so no message names its view and
no receiver has to defer one.

- `ToMain`, an mpsc FIFO in: `DispatchEvent`, `Resize`, `BeginFrame { now, seq }`,
  `Refill { offsets }`, `ImageEvents`. A FIFO because the order two commands
  arrive in is what they mean. `LynxView` holds the one strong sender, inside
  the seat an attached `Painter` holds only a `Weak` of, so a painter can never
  keep a released view's task alive.
- `ViewNotice`, an mpsc FIFO back, drained by `LynxView::pump`: `Engine(event)`,
  `RequestImages(sources)`, and `RequestSource { request, completion }`. Only
  what a host must *act* on rides here.
- `watch<Published>`, read by any observer and by the painter in particular:
  the newest committed frame, the listener-name set, and the newest serviced
  `BeginFrame`. Observed state rather than history — a painter wants the latest
  and never the ones it slept through.

One typed wakeup goes back to the embedder alongside them, and a view's worker
realms report on a fourth channel that belongs to the view rather than to the
link.

That main thread belongs to the `LynxGroup`, not to any one view. A group owns
it along with the one QuickJS runtime every view's realm is opened on and the
one Stylo pool every view's document restyles with; `create_lynx_view` is the
only way to build a view, because naming the group is the only way to say
which thread it runs on. One group per thread and one thread per group: the
handle is `!Send` and `!Sync`, so the thread that creates a group is the
thread every view in it paints on.

Views in a group take turns rather than run at once. Each has a set of tasks
of its own on that thread's `LocalSet`, one per thing it can wait for, so a
view waiting for its entry parks on its own channels and nothing it waits for
holds up a sibling — but every entry into a realm is one synchronous stretch
inside `Page::enter`, and only one task is inside the shared runtime at a
time. A second view therefore costs no second heap,
no second module graph and no second set of Stylo workers, at the price of
the two never restyling in parallel. The assumption that buys is that a
person drives one view at a time. A host that needs two pages genuinely
parallel gives them a group each, on a thread each. The pool in particular
*must* be the group's rather than any view's: rayon takes the calling thread
over as index zero of the first pool built on it and refuses a second one
there forever, so one thread can only ever build one pool.

Dropping a view cancels its source work, detaches its image inbox, and closes
its command channel, which is the goodbye its task ends on; that task releases
the view's realm — and with it the document and every worker the realm created
— and the thread goes on serving its siblings. The group's two threads
are joined when the group handle and the last view built from it are both
gone — a view holds its group alive as its last field, so the embedder may
drop them in either order.

```text
the thread that created the LynxGroup (AppKit main, or a Render Worker)
  window lifecycle, input capture, surface creation
  LynxView — the host's resource system, and the only turn that services it
  Painter — attached to that view; everything below runs inside the
  embedder's own calls:
    input routing + gesture recognition (against the adopted frame)
    scroll/dispatch/resize/BeginFrame
    compose: upload scene, acquire, present
    capture, offscreen ticks
  ── ToMain mpsc ──▶                  ◀── ViewNotice mpsc ──
                                      ◀── watch<Published> ──
                                          (frame, listener names,
                                           newest serviced BeginFrame)
                                      ◀── EventRequester wakeup ──
      Lynx main thread — the group's, shared by every view in it
                    (one task per wait; one runtime, one style pool)
                    the realm owns its document
                    PAPI mutations: plain &mut
                    __FlushElementTree: commit
                      style → layout → build → encode
```

The surface is built on that thread and stays there:
`create_surface` from a window handle panics off the macOS main thread, and
the same thread is the one that will acquire, render and present into it. That
is why the target is an argument to `Painter::new` rather than something
attached later, and why a `Painter` is `!Send`. A view may run with no painter
at all — it commits, and nothing draws — and a painter may outlive the view it
was watching, going on showing and capturing the last frame it drew. A frame's
vsync wait lands inside the embedder's own turn, which is why the
embedder draws where a wait for the display is acceptable rather than inside
every input relay. Nothing about this differs by platform any more: the
browser, where `wgpu`'s handles are not `Send` under shared memory and an
`OffscreenCanvas` cannot be transferred on again, always had exactly this
shape, and the Render Worker is simply the thread that constructs both.

A commit writes its frame into the watch, over whatever the painting side has
not read; the listener-name set and the newest serviced `BeginFrame` sit in
the same `Published` value, while lifecycle events and resource asks ride the
`ViewNotice` FIFO in order. Frames stay off that FIFO deliberately: a queue of
them would retain every intermediate scene, while a watch bounds the frames in
flight at one however far the main thread runs ahead. The painter adopts one
snapshot per pass — `poll_link`, run first by every entry point — and reads the
name replica, the pending-redraw bit, and the frame out of it for the rest of
that pass, so composing, hit-testing, and refilling take no lock at all. The
snapshot is *taken* rather than only read when the watch reports a change: a
completed `changed()` has already marked the value seen, so a flag alone would
skip exactly the state an offscreen wait was woken for. `Receiver::has_changed`
is never used either — it errors once the sender is gone, and the last frame a
view published before its task ended is still the frame the painter must draw.
Every publication from the
main thread wakes the embedder through its `EventRequester`, always after the
state it announces is in place, and the `Painter::pump` that answers is the
turn that draws it. A listener-name edge is the one exception that wakes
nobody: the painter reads the set at the start of its next routing pass, which
is a turn the host was taking anyway.
A frame the painter asks of *itself* wakes nothing either — it is asked
inside one of the embedder's own calls, and the turn that embedder is already
in is the turn that answers it. Nothing announces the main thread's exit:
dropping its end of the link closes the channels, which is the same fact, and
the painter notices a view that has gone on its next poll and detaches itself.
The view's own goodbye is its command channel closing, which is what its
command consumer ends the view on.

Every entry into a realm — input dispatches, scrolls, resizes, resource
updates, `BeginFrame` ticks, a module completion, a timer coming due, a
sibling's checkpoint — ends with a commit when anything went stale, which is
what makes the recorded contract true: script must flush after mutating, and
nothing guarantees the tree is *not* flushed at other times. A half-applied
JavaScript turn is still unobservable, because the epilogue runs after the
operation rather than inside it. A windowed painter
never waits on the main thread and never skips a frame: it always has the
latest adopted frame to compose and hit-test, however busy that thread is.
`Painter::tick` is the one call that does wait on it, and only an offscreen
painter has one — a host with no display to pace against asks for a frame and
waits out the `BeginFrame` acknowledgement, with a deadline, and ends early if
the view's task has gone.

## Scroll composes; a refill recommits

The frame is baked *unscrolled*: the walker's layer-stack pushes become a
compose program tagged with the scroll chain each shape rides, the content
between them lands in per-chain scene fragments, and replaying the program
with a set of per-slot offsets reproduces exactly what a monolithic encode at
those offsets would have produced. A user scroll therefore never waits for a
commit — or the main thread at all. The painting side arbitrates
consumption against the published slot table, keeps the consumed offsets as
*scroll intents*, and recomposes and re-hit-tests at those offsets
immediately; between refills the intents *are* the offsets, and no per-event
command exists. When a frame publishes, an intent the frame's own offset
already equals has served its purpose and drops; the rest re-clamp to the
new bounds.

The encode is windowed: each slot's fragments cover one scrollport past its
committed offset per scrollable axis (`ENCODE_WINDOW_SCROLLPORTS`). When an
intent moves past half its remaining window headroom, the engine sends one
`Refill` per committed frame carrying the offsets the screen is showing;
the main thread writes them into the document, marks the paint stale, and
its next commit re-bakes the windows centered on them — no script
involvement anywhere. The refill write-back is the only way a user scroll
reaches the document, so between refills document-side offset reads lag the
screen; a future script-facing scroll API must either dirty the paint or
publish its offsets, since the compositor only knows what crossed the
channel.

### Scroller content lives in retained planes

A scroll container is a forced stacking context (Lynx's native scroll views
are compositing boundaries; recorded deviation from the web, where
`overflow` alone creates none), so its subtree encodes as one contiguous
program run. At commit, the painter partitions the program into a
*composite plan*: maximal contiguous runs riding one scroll head become
*planes* — each baked unscrolled into a GPU texture covering its scrollport
plus encode window — and everything else (root content, which viewport
culling already bounds by the screen; animation-chained content; groups the
bake rules refuse) stays raw. Both outputs keep a `PlaneBank`: a new commit
re-bakes the planes' textures; every frame after that composes raw steps
plus one textured draw per plane, each under its slot's clip chain. A
scroll frame therefore re-encodes and re-rasterizes none of the scroller
content — its whole cost is the raw steps, the plane draws, and vello's
per-use copy of each plane texture into its image atlas. Plane memory is
screen-proportional — scrollport-sized windows per scroller, never
per-fragment — and capped at half vello's 8192×8192 atlas; a frame past
the budget, and any frame recommitting every tick for an unexported
animation, plans nothing and composes flat exactly as above. No frame
materializes a whole composition beside its fragments (that would be a
content-proportional second encoding): `scene()` borrows the single
fragment of the common whole-frame shape and answers `None` for every
other, and consumers needing a flat scene compose one on demand.

## Composite animations compose; the rest tick

The same compose machinery carries animations. At commit, an element whose
one running animation moves only `opacity`/`transform` — and whose keyframes
and structure the exporter can re-express exactly (see
`docs/tracking/css-animation.md`) — publishes an `AnimationSlot` curve on
the frame: timing from stylo's public `Animation` fields, per-property
tracks re-read from the stylist's `@keyframes` steps. The element is forced
to paint as a stacking context with a composited group, its subtree's
fragments and layer pushes are tagged with its animation chain, and each
presented frame samples the curve at the frame clock: the group's alpha is
replaced, and the transform delta against the committed bake multiplies
into the tagged fragments, pushes, and hit tests. Between commits the
compositor animates alone.

`BeginFrame` narrows accordingly: it is sent per frame only while the
committed frame reports `needs_main_ticks` — something animating that could
not export — and once when a finite curve runs past its end, so the main
thread runs the finish restyle and commits the end state. An infinite
exported animation involves the main thread zero times per frame. The
sampling mirrors stylo's own progress computation exactly, so the values
composition shows between commits are the values any commit's restyle
lands on at the same instant — handoffs are seamless in both directions.

## Native and Wasm spawning

`LynxGroup::new` starts the group's two engine threads, and `create_lynx_view`
hands each view to a task on the first of them. The core selects the thread
builder at compile time:

```text
not wasm32  -> std::thread::Builder
wasm32      -> wasm_thread::Builder
```

Both engine threads run a tokio `current_thread` runtime under a `LocalSet`,
and both need to wait out their realms' timer deadlines. Natively they enable
tokio's time driver. On wasm32 that driver reads `std::time::Instant`, which
panics there, so `crate::clock::sleep_until` is served instead by `alarm.rs`:
one process-wide `bobcat-alarm` `wasm_thread` Worker holding a heap of
deadlines and the wakers waiting on them, parked with `park_timeout`, which
needs no clock the platform does not have. It is started by the first group's
`bobcat-main` for the same reason a group starts its own two threads eagerly —
a card's first `setTimeout` must wait out its own delay rather than a Worker's
cold start. The tokio feature set follows: `rt`, `sync` and `macros`
everywhere, `time` only under `cfg(not(target_arch = "wasm32"))`.

On Wasm, `configure_wasm_workers(worker_script_url)` is the OS bootstrap
seam. It configures the default `wasm_thread` worker script and nothing else;
the core then uses that same target-specific spawn path for each group's Lynx
main Worker, its worker-realm Worker, the alarm Worker, and that group's Stylo
Rayon Workers. Stylo pool creation
belongs to the core, not the browser facade — the facade only says how many
workers a group should get, as the `StyleThreads` argument to
`LynxGroup::new`.

Wasm and native follow the same ownership model: every `LynxGroup` spawns and
owns one Lynx-main Worker and one worker-realm Worker — the Render Worker is
the thread that created the group, so it is where the painting happens — and
dropping a view ends its task, which releases that view's realm, document and
workers, while dropping the last of the group's handles is what joins the
Lynx-main Worker and then, once its senders are gone with it, the worker-realm
one.
Independent groups are
not a process-global singleton. The npm facade keeps at most one live view,
in one group, per `BobcatRenderer`, and **one `Painter` for the canvas across
all of them**: `create` builds no group and no view, and each `load` detaches
the painter, replaces both the view and its group, and re-attaches. The
painter, Render Worker, transferred
canvas, Wasm instance, and wrapper state are retained; the canvas is not
resized on a load, so the previous page's last frame stays up while the next
one boots. A page gets a group of its own
because the script runtime is the group's, so a page loaded twice would
otherwise register its entry module a second time under a name the previous
load already took. Replacement construction therefore cannot overlap the old
view's teardown.

Nothing about Stylo is process-wide any more. Each group's Lynx-main Worker
builds one Rayon style pool as its first act of startup, before any view
attaches, and the pool retires with the group. That Worker is index zero of
the pool it builds, taken over in place by rayon's `use_current_thread` —
which is why the pool can only be built there, why the configured count
includes it, and why the pool has to be the group's: a second one on the same
thread is refused outright, so every view the thread carries shares that one
or has none. Stylo's global pool
was built the same way and Gecko relies on it, so a lone view restyles on the
same threads and with the same parallelism it did before these pools became
per-view: the root closure runs inline on the script owner, and the managed
members take over only where a level is wider than the traversal's work unit.
The takeover is permanent — rayon leaks about 25 KB per pool and refuses a
second pool on the same thread forever — which is affordable only because the
Lynx-main Worker is created for one group and dies with it. Disjoint pools are
what let two views on two threads restyle at once; the process-wide traversal
mutex that serialized them is gone. `dom::MAX_STYLE_THREADS` (six) is a hard
ceiling, not a preference: Stylo indexes its per-traversal thread-local
storage by Rayon thread index into an array that long, and counts its own six
the same way.

The browser UI thread is a JavaScript coordinator only. It creates an
embedder/Render Worker and transfers an `OffscreenCanvas`. That Worker owns
the Wasm `LynxView`, its `Painter`, the Vello/wgpu objects, and the resource
provider; core creates
its owner-thread-bound realm inside the nested Lynx main Worker, and the
realm's own boot module creates the document there. No direct
create/append/drop/flush DOM API is exposed to JavaScript.

## Frame walkthrough

1. `LynxGroup::new` starts both of the group's threads — `bobcat-workers`
   first, then `bobcat-main`, which is handed one sender on it — and waits for
   `bobcat-main`'s report that the group's QuickJS runtime and Stylo pool are
   built. `create_lynx_view` validates the metrics, creates the view's link,
   sends the far half of it to that thread, builds the per-view
   `ResourceFetcher` on the calling thread, and returns a loading view
   synchronously. The embedder then builds a `Painter` over the `DrawTarget` it
   named and attaches it, which imposes the painter's metrics on the view.
2. The view's task validates the fonts and default family into a
   `dom::TextContext`, then requests each stylesheet in cascade order and the
   entry MTS source, staging each answer in `DocumentIngredients`. Ordinary
   `LynxView::pump` turns hand those requests to the fetcher and its
   completions answer the tasks awaiting them.
3. On entry arrival the task opens the realm, installs the host members, and
   evaluates `bobcat:boot`. Boot's first statement constructs the realm's
   `Document`, which is what builds the private document out of the staged
   ingredients — style pool, text context, sheets in cascade order, buffered
   image reports — before the entry loads. `ScriptFinished` or `StartupFailed`
   reports the outcome through the lifecycle event path. Dropping the view
   cancels its pending resource work; other views continue.
4. `__FlushElementTree` commits — style flush, layout, paint-order build,
   scene encode — publishes the `Arc<CommittedFrame>` on the view's
   `watch<Published>`, and wakes the embedder through its `EventRequester`.
5. The host answers with two turns. `Painter::pump` adopts the newest
   `Published` together with the pixels it draws and produces the frame: the
   `FrameClock` is sampled once, gesture deadlines resolve against it, the
   adopted scene is uploaded if it is new, and the frame
   presents. `LynxView::pump` then services the host's resource system and
   hands back the lifecycle events. While the latest frame reports an active
   animation each painter turn
   sends the main thread one `BeginFrame` carrying that reading, and the loop
   sustains without any JavaScript and without waking anyone: `owes_frame`
   answers yes, and the embedder takes the next turn at its own display frame
   — a `CVDisplayLink` on the window's monitor natively, `requestAnimationFrame`
   in a Worker. The engine names no interval, and an offscreen host, which has
   no display to pace against, reads `Painter::is_animating` instead.
6. The successful boot notification remains queued behind the same wakeup;
   the awakened host observes it through `LynxView::pump`, which hands it back
   with whatever else the turn produced. A draw that fails is the return value
   of `Painter::pump`, `tick` or `capture`, reported once and never again. No
   realm or tree object crosses the boundary.

## Validation matrix

```sh
cargo check -p bobcat-core
cargo check -p bobcat-core --target wasm32-unknown-unknown
cargo check -p bobcat-source
cargo check -p bobcat-cli --no-default-features --features cli
cargo check -p bobcat-cli --no-default-features --features server
cargo check -p bobcat-wasm --target wasm32-unknown-unknown
cargo check --workspace --all-targets
```

The two wasm32 lines take no `--all-targets`, and CI's `browser` job lints
that target with `--lib` for the same reason: `bobcat-core`'s library builds
there — its tokio features are target-gated, with `time` enabled only off
wasm32 — while its *dev* dependency asks for `rt-multi-thread`, which does not
compile for wasm32 at all and which feature unification would drag into any
build that includes dev targets.
