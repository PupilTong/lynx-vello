# Runtime architecture

Bobcat exposes two runtime objects to an embedder: `bobcat_core::LynxView`,
which is a page, and `bobcat_core::Painter`, which is where a page's pixels
go. They are built separately on the same embedder thread and joined at
runtime by `Painter::attach`. The document, Element-PAPI tree, script realm,
and the commit/publish protocol are implementation state. An embedder supplies
only capabilities and OS facts:

- a `ViewSources` — the required base URL the view's entries and every
  realm's synchronous loads resolve against, page config, owned font bytes, an optional default font
  family, author stylesheet URLs, the one entry MTS
  module URL, optional `init_data` and `global_props` JSON text, and the
  required `screen` metrics `SystemInfo` reports — and,
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
  realm/mod.rs         open_realm, the one constructor every realm is opened
                       with, and the core every realm gets from it
  realm/owner.rs       the one driver a view's page and a worker share: the
                       entry boundary, the epilogue, module loads, future
                       settles, the end and the release
  realm/policy.rs      what a failure is reported as: one table per realm kind
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
  main/page.rs         one view's page: its tasks — the entry, commands,
                       metrics, worker events, fonts, timers, checkpoints —
                       and what a page adds to the realm driver
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
since nothing in it belongs on the embedder's thread. The task keeps the
Rust-side document inputs — the create-time viewport, the page configuration,
the validated text context and the group's style pool — as
`DocumentIngredients`, and collects the rest — the author sheets' answers,
the screen, the BTS entry, the page data, the processor name and the module
table — into one `RealmStartup` that opens the realm. The entry's answer is
not part of it: it is a task of the view (`load_entry`) that enters the realm
when its answer arrives. **Nothing waits for any of it before the realm
opens.** The page configuration, the screen and the BTS entry are written
into the boot module as literals; the sheets' answers go to the realm's
`DocumentSlot`, in listed order, and boot's first `__FlushElementTree` waits
for each and mounts it before the document is styled; the entry completes the
module boot imports it as, by its URL. `LynxView::update_data`,
`update_global_props` and `reload` reach the realm through `ToMain::PageUpdate`
afterwards and never touch any of it.

`ViewSources::screen` is the screen `SystemInfo` describes — `pixelRatio`,
`pixelWidth` and `pixelHeight` — as the embedder measured it: web-core's
algorithm (`devicePixelRatio`, and `screen.availWidth`/`availHeight`
multiplied by it) in a browser, the monitor the window is on natively. It is a
screen rather than a view, so the view's own viewport is not an answer to it.
It is required: a host with no screen to measure — a headless or offscreen
capture — names `ScreenMetrics::for_viewport(width, height,
device_pixel_ratio)` of its capture size explicitly (`pixel_ratio` is that
ratio, and the two sizes are the CSS size multiplied by it), and nothing in
the engine derives one on a host's behalf. It reaches the boot module as three
JavaScript number literals; nothing updates it afterwards, so a painter that
binds at other metrics leaves it alone. The BTS realm gets the MTS realm's
own `SystemInfo`: `__BobcatConnectBackground` posts it to the BTS in the
`initialize` message, and `bobcat:bts-runtime` builds its `SystemInfo` out of
it before it imports the BTS entry. The numbers are the ones the boot
module's literals read as, so a ratio an `f32` cannot hold exactly is the same
number in both realms. A worker at any URL other than `bobcat:bts` that
imports `bobcat:bts-runtime` is posted no `initialize`, and its `SystemInfo`
is the runtime constants alone.

`ViewSources::init_data` and `global_props` are optional JSON text, and Rust
never reads it. `MainThreadRuntime::new` puts each behind a
`bobcat-internal:host` member of its own, `initData` and `globalProps`, which
hands the string over once, as a plain string. `bobcat:runtime` calls both as
it evaluates and parses them: the global props become `__globalProps` and
`lynx.__globalProps`, and the init data becomes `__BobcatInitData`, which boot
hands to `processData`. A value that was not given arrives as `undefined` and
is `{}` there, as in web-core. Text that is not JSON fails boot with
`StartupFailed`, naming the input, before the entry runs. The BTS receives
both from the MTS realm, in the `initialize` message: the global props as the
MTS realm holds them, and the init data as `processData` returned it.

The embedder's native modules reach the BTS realm in the `initialize` message
as well. `LynxGroup::create_lynx_view` reads each module's `name()` and
`methods()` once and encodes them as one length-prefixed record;
`MainThreadRuntime::new` puts it behind the one-shot `bobcat-internal:host`
member `nativeModuleTable`, beside `initData` and `globalProps`.
`bobcat:runtime` reads it as it evaluates and never decodes it:
`__BobcatConnectBackground` posts it to the BTS in `initialize`, and
`bobcat:bts-runtime` builds `NativeModules` out of it before it imports the
BTS entry. So `initialize` carries the page's data, the BTS entry's URL, the
MTS realm's `SystemInfo` and this record, and the BTS's `WorkerStart` carries
none of the view's data: it differs from any other worker's only in its URL
and its `ScriptSource`. A plain `Worker` is posted no `initialize`, so its
`NativeModules` is empty. Every realm kind declares the host module
`bobcat-internal:native-modules`, with `invokeNativeModule` alone, so the
transport `bobcat:native-modules` links in each. The modules
themselves never leave the embedder's thread: a call arrives back as
`ViewNotice::NativeModuleCall` — the calling realm (`None` for the MTS realm,
the worker's key for a worker), the call's text and the indices of its
function arguments, nothing built — and `LynxView::pump` assembles the
`ModuleCall` there, over a weak handle `FrameDemand::reply` picks by that
caller: the calling worker's inbox, which `ViewNotice::WorkerCreated` already
registered, or the view's own command sender. It then hands the call to the
module of that name. An answer goes back as `WorkerMessage::ModuleCallback`,
which the worker delivers at once, or as `ToMain::ModuleCallback`, which the
MTS realm applies in the view's next command burst; both call
`bobcat:native-modules`' `__BobcatNativeModuleCallback` in the realm that made
the call. Because the MTS answer is a command, it waits while a job of the
view is parked on a synchronous wait, and after a fatal event a callback reads
as cancelled only once the view's task has ended.

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

QuickJS ESM graph — an MTS realm, on bobcat-main's runtime
  bobcat:boot
    ├──▶ bobcat:element (Document class + flush binding)
    ├──▶ bobcat:timers (timer-global installation)
    ├──  const config = { defaultDisplayLinear: …, … }  four boolean literals
    ├──  export const document = new Document(config)  the realm's first
    │      └──▶ bobcat-internal:host.createDocument      statement
    │            └──▶ four booleans + DocumentIngredients ──▶ private dom::Document<()>
    │   (the author sheets are mounted on this document, in listed order, by
    │    the first __FlushElementTree below, never by a statement here)
    └──▶ await import("<entry URL>")    completed by the view's `load_entry`
          │                               task from the pre-issued answer
          └──▶ the entry, with import.meta.url = its response URL; before
               completing it, the task calls __BobcatInitEntry(response URL),
               which names __Card__
          ├──▶ bobcat:runtime (packages/bobcat-element/src/main-thread-runtime.ts)
          │     ├── named compatibility exports + engine EventTarget
          │     ├──▶ bobcat:cross-thread-context (MTS getJSContext)
          │     ├──▶ bobcat:diagnostics (packages/bobcat-element/src/diagnostics.ts)
          │     │     └──▶ bobcat-internal:host (reportScriptError, logScriptMessage)
          │     │           console and _ReportError, re-exported as module bindings
          │     └──▶ bobcat:event-target (packages/bobcat-element/src/event-target.ts)
          ├──▶ bobcat-internal (explicit import; Worker class in worker.ts)
          │     ├──▶ bobcat:event-target
          │     └──▶ bobcat-internal:host (createWorker, sendWorkerMessage, terminateWorker)
          └──▶ bobcat:element (packages/bobcat-element/src/element-papi.ts)
                └──▶ bobcat-internal:host (native named function exports)
                      └──▶ the document created above

QuickJS ESM graph — a worker realm, on bobcat-workers' runtime
  <URL> (the root module: the module at the worker's URL itself, loaded as
    │    the realm opens at its Start, the way import("<URL>") loads one;
    │    the engine writes nothing around it and installs no global scope)
    ├── a URL outside the engine  completed by the worker's consume_messages
    │   prefixes                  task, under the request URL, from the
    │                             answer to the request createWorker made;
    │                             the script runs with import.meta.url = its
    │                             response URL, and imports bobcat:worker and
    │                             bobcat:timers itself when it uses them
    ├── an engine name            loaded by the realm's own loader, or
    │                             refused there with a ReferenceError; never
    │                             requested
    └── bobcat:bts                the BTS: a registered module (bts.ts)
          ├──▶ bobcat:worker (packages/bobcat-element/src/worker-runtime.ts)
          │     ├── the global scope: self, postMessage, close, name (read
          │     │   from workerName in its last statement), onmessage, console
          │     │   (no requestAnimationFrame)
          │     ├──▶ bobcat:event-target
          │     ├──▶ bobcat:diagnostics ──▶ bobcat-internal:host
          │     │     (reportScriptError, logScriptMessage), the global console
          │     └──▶ bobcat-internal:worker (postWorkerMessage, closeWorker,
          │                                   workerName)
          ├──▶ bobcat:timers ──▶ bobcat-internal:host (setTimer, clearTimer
          │                       only)
          ├──▶ bobcat:bts-runtime exports lynx; the `initialize` message
          │     fills SystemInfo and NativeModules (its table read through
          │     bobcat:record) before the entry is imported
          │     ├──▶ bobcat:native-modules (callNativeModule, the transport
          │     │     each NativeModules method calls) ──▶
          │     │     bobcat-internal:native-modules (invokeNativeModule)
          │     ├──▶ bobcat:diagnostics (console, lynx.reportError; the
          │     │     console export is the global one)
          │     └──▶ bobcat:cross-thread-context ──▶ bobcat:event-target
          └──▶ await import(the entry URL `initialize` names), when the view
                named one: HostOutbox → view resource host → worker
                completion
  A realm in which bobcat:worker never ran has no global scope: a message
  posted to it is dropped, with nothing reported.
  Both runtimes register the same twenty-one built-ins (esm.rs BUILTIN_MODULES),
  and a realm's host modules decide which of them link. Here bobcat:element,
  bobcat:runtime and bobcat-internal fail at link with a SyntaxError: they
  import bobcat-internal:host members only an MTS realm has. In an MTS realm
  bobcat:worker, bobcat:bts-runtime and bobcat:bts fail to load with a
  ReferenceError: they import bobcat-internal:worker, the last two through
  bobcat:worker, which it does not declare.
  bobcat:native-modules links in both: every realm kind declares
  bobcat-internal:native-modules. Any other bobcat: or bobcat-internal: name,
  one no runtime registered and no realm declared, fails its import or
  require in the realm with a ReferenceError and is never sent to the
  fetcher.

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
its only home. A window painter's draw sends one `BeginFrame { now, seq }`
command per frame while the latest committed frame reports an animation it
cannot compose alone (see "Composite animations compose; the rest tick");
an offscreen `tick` sends one on every call. The view's command consumer
advances the timeline — a Stylo animation-only traversal of just the
animating elements, no JavaScript involved — and commits what changed. The
`seq` is what an offscreen `tick` waits on: the
epilogue publishes the newest serviced sequence number on the view's watch
after the commit it implies, so a host blocked on that number is woken by the
frame rather than by the acknowledgement. The published frame's
`animations_active` flag is what keeps the loop sustained: `owes_frame` keeps
answering yes, the embedder keeps taking a turn per display frame, and each
one that owes the main thread a tick sends `BeginFrame`, until a commit reports
the timeline idle. Starting and cancelling animations belong to the style
flush the main thread already runs at `__FlushElementTree`.

Because that is the whole supply of timeline readings, an idle page's timeline
stands still: the flush that creates an animation reads whatever the last
`BeginFrame` left, which may be many seconds old. So the flush only arms an
animation — the first `BeginFrame` after it is what starts it, and the driver
shifts the pending start time onto that frame's reading (see
`crates/dom/src/style/animation.rs`). A tap that starts a four-second animation
after ten idle seconds plays all four seconds.

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
initializing `lynx.getCoreContext()`. Worker ESM imports use the view's
ResourceFetcher; MTS top-level await is part of startup readiness, and the BTS
entry's is not. Compiled
bundle factories still need the module/init shell from a later stack layer.

`LynxGroup::new` awaits the shared style pool. A script runtime that
`build_runtime` could not build does not fail the group: each view reports it
as `StartupFailed` when its realm would open, and each `Worker` as `Failed`.
`create_lynx_view` sends the view's half of its link to the group's thread and
builds the host's fetcher on the calling thread. It is synchronous — nothing it
builds can block — and returns a loading view. Attachment, native-module and
font failures are returned by construction; everything else is a lifecycle
event on the view it returns.

Its `width`, `height` and `device_pixel_ratio` are the **create-time
viewport**: the metrics this view's document is built at and works at until a
painter binds to it. They are not validated, because no draw target is built
from them; the painter's own metrics are, by `Painter::new` and
`Painter::resize`, and they supersede these the moment one attaches. A
create-time viewport equal to the painter's is the one that costs nothing —
see [Document and rendering ownership](#document-and-rendering-ownership) for
what a mismatch costs.

**The view's startup sources are requested inside `create_lynx_view`.** The
entry and the BTS entry are resolved first, against `ViewSources::base_url` by
URL rules (so `main.js`, `./main.js` and `/main.js` are all relative URLs), and
replaced by the WHATWG serialization of the result: a base that is not an
absolute URL, or an entry that does not resolve against it, is a zero-fetch,
synchronous `EngineError::InvalidUrl` naming the string that failed. The
parsed base then crosses to `bobcat-main` in the attachment, and every realm of
the view resolves its synchronous loads against it. The base itself is not
handed to the fetcher, which resolves stylesheets and fetches against a
base of its own; an embedder must give it the same one, and every embedder in
this workspace does. Fonts
and the default family come next, validated against a `dom::TextContext` of
their own: they are a text context's business, no document exists yet, and an
unknown default family is therefore a zero-fetch, synchronous
`EngineError::UnknownFontFamily` rather than a later `StartupFailed`. Then each
author stylesheet in the order the view listed them, by the string it was
listed as, then the entry, by its resolved URL, handed straight to
`ResourceFetcher::request_source` on the embedder's own thread — the fetcher
was built a few statements earlier in this same call — with the answering
one-shots crossing to `bobcat-main` inside the attachment. That is what makes
the IO and the view's whole boot overlap the embedder's next act, which in
every embedder is building this view's painter: the reference fetcher queues
its job on its own pool (a browser task on Wasm) and needs nothing from the
view, and `bobcat-main` opens the realm, runs the entry and encodes its first
frame without a host turn. Every *later* source request rides a
`ViewNotice::RequestSource` carrying the request and the right to answer it,
which `LynxView::pump` hands to the fetcher: imports, `adoptStyleSheet`,
worker scripts, fonts and plain fetches.

**Order of completion is the fetcher's; order of use is the view's.** The
entry is read by a task of the view's owner as its answer arrives. The author
sheets are read by boot's first `__FlushElementTree`, one at a time and in the
order the view listed them, whatever order they were answered in, so **the
cascade order between several listed sheets is the listed order**, as in
web-core. The first failure to reach the realm ends the view, and later ones
are not reported.

Either way the fetcher resolves the URL — a stylesheet's against its own base;
the entry's, like every script request's, is already absolute — fetches bytes
and validates UTF-8, or
supplies a pre-parsed stylesheet. Completion consumes the handle and answers
the one-shot minted with the request, which wakes whichever task was awaiting
that source — a stylesheet or entry on `bobcat-main`, a worker script on
`bobcat-workers` — without a turn anywhere else. The handle contains that
sender and a clone of the view's `CancellationToken`: no erased callback,
retained resource Future, `SourceLoads` or `EventWaker` is needed. The
fetcher itself is owned by value and needs neither `Send`, `Sync` nor
`'static`. A view's task awaits no IO on any other view's behalf, so a sibling
can boot or handle events while this view loads.

**No startup source parks anything before the first flush.** `createDocument`
builds the document and returns. Boot's first `__FlushElementTree` is where the
author sheets are settled, **before the document enters the style pipeline**:
for each sheet in listed order it parks the job on that sheet's answer —
`JsThread::wait`, the view's token as the biased first arm, and no wait at all
for an answer already in hand — and mounts it with the same code
`adoptStyleSheet` mounts an answer with. The sheets need no task of their own:
the fetcher answers the one-shots from its own pool, so nothing on
`bobcat-main` has to run for them to arrive. Until every listed sheet has
settled the epilogue's implicit commit (`commit_if_dirty`) does nothing, and
never waits: it runs after every entry, and parking there would stop the group
on any of them, while a commit without the sheets would publish an unstyled
frame. Nothing is lost by skipping it, because boot's own flush is what commits
the first frame and it settles the sheets first. So a view publishes nothing —
no frame and no `ScriptFinished` — until every listed sheet has loaded or
failed. Boot imports the entry by the URL
`create_lynx_view` resolved, which is absolute, so the module normalizer maps
it to itself. The entry's task (`load_entry`) awaits
its answer and completes that module with it, exactly as the fetcher answered
it and exactly as an ordinary import is completed: the module is
registered under the request URL, which is the name its errors carry, and
answered from the fetcher's response URL, which is its `import.meta.url` and
the base its relative imports resolve against. Boot's `import` finds it in the
registry if it was completed first, and is resumed by the completion
otherwise; the entry's own request is answered by `load_entry` and never
reaches the fetcher a second time, so the epilogue skips it. Nothing is added
to the entry: the imports a card's MTS body is given, `MTS_CHUNK_PREAMBLE`,
are `bobcat-source`'s to prepend, which it does to every card body it
registers, on the body's own first line, so a line of the entry is the line
its errors report. An entry that is not a card's body imports what it uses
itself. The only two
things boot waits on are both inside its first flush: the listed sheets, then
a painter's binding (see
[Document and rendering ownership](#document-and-rendering-ownership)). While
that flush is parked no other job of the group runs, as for any synchronous
host member; the sheets were requested inside `create_lynx_view`, so the wait
is for IO already in flight, and the painter's construction overlaps it.

Failures are reported by where they happen. An entry that fails to load is
read as such by `load_entry` before any of it runs, and the embedder is told
`StartupFailed` carrying the fetcher's own error; an answer that is not a
script, or a script whose response URL is not an absolute URL, is
`StartupFailed(LynxViewError::Script(..))` naming the URL. That response URL
becomes `__Card__`, the base every `new Worker` URL is joined to, boot's
`bobcat:bts` included. Neither completes the entry's module: the view has
ended. A sheet that fails to load,
or that the fetcher answered with something other than a stylesheet, makes
`__FlushElementTree` throw `loading stylesheet <url>: <reason>`: boot's own
flush rejects boot, so the embedder is told
`StartupFailed(LynxViewError::Script(..))` naming the sheet — after a
`ScriptRunError` with the same message, from the entry whose checkpoint
returned that rejection — and a flush the card makes itself throws to the
card. `DocumentSlot` keeps the first listed sheet that failed and throws it
from every later settle, so a card that settled the listed sheets first, by
an `adoptStyleSheet` or its own flush inside the entry's evaluation, and
caught the failure still leaves boot's own flush to fail the boot with it.

**An entry that throws does not fail the boot.** The entry is app code: boot
imports it inside a `try`/`catch` whose `catch` raises the error again as a
rejection nothing handles, so the checkpoint of the entry that ran the
`catch` reports it once — `ScriptRunError` from `load_entry`, `load_module`
or `settle_future`, `TimerFailed` or `ListenerFailed` where the entry's
top-level `await` resumed in a timer or a worker event — and boot goes on to
connect the BTS, render and flush, so `ScriptFinished` still follows its
flush. A native MTS whose top-level script throws also goes on to render the
page. What fails boot's own evaluation is the engine's code alone:
`bobcat:runtime` reading page data that is not JSON, the document's
construction, connecting the BTS, and the flush. `processData` and render
hook failures never reach it either: `main-thread-runtime.ts` catches them
and reports a `ScriptReported`. What Rust
keeps for itself is `DocumentIngredients` — the create-time viewport, the
`PageConfig`, the validated text context and the group's style pool — which
`MainThreadRuntime::new` puts in the realm's `DocumentSlot`.

The response carries a loaded source or error. Main owns the boot outcome:
`ScriptFinished` reports success; `StartupFailed(LynxViewError)` reports resource,
encoding, realm or boot failure exactly once through `LynxView::pump` — a font
failure is not among them, having already refused the construction.
A failed view asks its host for nothing more, sources and images alike.
Its resolved entry URL is the module specifier.
`ScriptRunError` reports main-thread app code that threw, or a host call into
the realm that failed, during boot or after it; like listener and timer
failures, it is non-fatal and the realm goes on. `Panicked` reports an engine
panic and, with `StartupFailed`, is what `EngineEvent::is_fatal` names: the
two events that end a view. A host member that panics is an engine panic too:
the bridge cannot unwind through `QuickJS`, so `ScriptEngine` keeps the panic,
the script is shown the exception "the host function panicked", and the
realm's next checkpoint — normally the one that ends the entry that called
the member — resumes the panic, whether or not the script caught that
exception. Every main notification requests a host turn
through `EventRequester`.

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

Source requests select a module or stylesheet payload. A
`SourceRequest::Module` — the entry, an import, a worker script or a
synchronous load — carries an absolute URL Rust resolved, in its WHATWG
serialization; a font carries the absolute URL the document resolved; a
stylesheet or fetch carries the URL as it was named, and the fetcher resolves
it against its own base, which must equal `ViewSources::base_url`. The fetcher owns transport policy. That call, the optional
`preload_source` hint, `request_image`, `service_images` and the `FrameImages`
supertrait are the whole protocol. Every method is synchronous — no transport
future crosses this interface, and core
names none of a fetcher's own transport API. The protocol carries no
response-size limit; each fetcher owns the bound for the response it
materializes.

An engine thread is `jobs.rs`'s `JsThread`: a tokio `current_thread` runtime
with a `LocalSet`, and a FIFO of jobs its top loop runs *between* two turns of
that scheduler. The split is the execution model. **Tasks wait and route**; they
never run JavaScript, touch a document or borrow the shared `ScriptRuntime`.
**Jobs are the only place any of that happens**, and because the loop calls them
outside every `block_on`, a job may block: `JsThread::wait` is a fresh
`block_on` over the same `LocalSet`. While a job is parked on one, the
scheduler keeps going — channel reads, lifecycle signals, acknowledgements,
resource routing, other realms' timers — and no other job runs. Jobs pushed
meanwhile queue behind the parked one and run in order once it returns, which is
why an entry may hold both its borrows across its own wait.

A view is a set of tasks on that `LocalSet`, one per thing it can wait for, and
tokio owns the polling, parking and waking. `serve_view` is the owner and has
exactly one wait of its own — the view's end; it queues the realm-opening job
before it spawns anything and waits for none of the view's sources;
`load_entry` completes the entry when its answer arrives; `consume_commands` is
the one ordered consumer of the command channel; `consume_metrics` settles the
page once per change of the painter's metrics; `consume_worker_events` is the
one ordered consumer of this view's workers; the epilogue spawns one
`load_module` per resource load an import produced, one `settle_future` per
`Future` a `.then` asked the realm to settle, and one `load_font_face` per
`@font-face` rule a mounted sheet declared; `serve_clock` owns the realm's one
pinned sleep and watches the runtime-wide checkpoint generation. Nothing is
spawned per input: an ordered stream stays serial because one consumer reads it
with `while let Some(x) = rx.recv().await`.

Every one of them reaches the realm through `enter` in `realm/owner.rs`, the
one JavaScript execution boundary, which a view's page shares with every worker
(see [Realm construction and driving](#realm-construction-and-driving)). It
queues a job and answers with what that job returned. The job runs one
synchronous operation under the borrows of the shared runtime and the realm,
and then the epilogue. The epilogue is one function for both kinds of owner:
the steps both have are written in it once, and what only a page has is a hook
of the page's `RealmOwner` impl, run at a fixed place among them. For a page
the order is — due timers first, because whatever just ran may have armed or
cleared one and its mutation should ride the same frame (the page's hook also
ends the batch those callbacks ran, which runs the collection their removals
may have made due); the commit next, so the frame exists before anything
implying it; then the two batches of engine-decided events that commit may have
left owing — `contentvisibilityautostatechange` and an `<image>`'s
`load`/`error`, each posted as one fresh entry rather than run here, so a
handler's own mutation gets a commit of its own — the boot report, the
`BeginFrame` acknowledgement, the module requests the operation left other than
the MTS entry's own, the futures it asked to settle, the `@font-face` loads its
sheets declared, the next timer deadline republished only when it moved, and
finally the checkpoint generation as of this entry. The commit and the two
posts are the page's `after_timers` hook, `ScriptFinished` its `on_booted`, the
acknowledgement its `after_boot` and the font loads its `after_settles`.
`Settles::settle` is the epilogue alone, for a wake that carries no operation
of its own; `Settles` is one blanket impl over every `RealmOwner`.
`Page::open_realm` is a job too and the only one outside `enter`, because the
realm it would enter does not exist until it returns; it runs the first
epilogue itself once it has stored the realm. The disposal exchange the page
runs as its `before_release` hook is the other, running past the latch and the
epilogue because the view has already ended. A command opens a burst: the rest
of what is already queued goes with it, bounded by the length the count was
taken from, so a host's whole round of input is one entry, one commit and one
acknowledgement rather than one of each per command. The consumer awaits that
burst's job before reading the channel again, so what arrives meanwhile is one
later burst.

**Nothing of a view is served outside a job.** Opening the realm is that
view's first job, queued before its own tasks exist, so a burst that arrived
before the realm did is a job queued behind it and finds a document. The cost
is that a `BeginFrame` is acknowledged by a job too: while any job of the group
is parked — an entry's `adoptStyleSheet` or `require` among them — the
acknowledgement waits with it. Boot parks for its startup sources only
inside its first flush, on the listed author sheets that have not arrived; the
entry is a task of the view that enters the realm when its answer arrives.

That checkpoint watch is a runtime-wide `u64` bumped inside
`ScriptEngine::checkpoint`. The promise-job queue belongs to the runtime rather
than to any realm, so a view whose import finished inside a *sibling's* entry
into JavaScript has to settle what its own realm owes; the checkpoint arm of
`serve_clock` is how it learns to, and comparing the generation against the one
the epilogue recorded is what keeps a page's own entries from waking it. The
generation is also bumped, by a job, whenever a view task on `bobcat-main` or a
worker task on `bobcat-workers` ends, whether it returned or panicked: a task
that ended part-way through may have left the shared queue with work in it, and
a panic inside one of its jobs is caught there, after which the task returns
normally. The bump settles every other realm on that runtime once; it does not
drain the queue itself.

An end is one signal rather than a message anything has to race. A view and a
worker are both built from `lifetime.rs`'s `Lifetime`: the `JoinSet` holding
that object's tasks, the `CancellationToken` that ends them, a thread-local
latch, two report latches, and the deadline and checkpoint generation that
object's one `serve_clock` task reads. The driver's `end` — the command channel
closing, a cancelled load, a startup failure, a panic in any task — sets the
latch synchronously, cancels the token and withdraws the armed deadline, and
the call that did so then runs the owner's `on_end` hook: a page's
acknowledges whatever `BeginFrame` was pending, so a painter blocked on it is
released rather than left waiting for a frame that will never come. That holds
for every way a view ends, a `StartupFailed` or a `Panicked` as much as a
release. Every entry point returns at once when the latch is set; the owner
(`run_owner` in `realm/owner.rs`), whose one wait is the token versus the next
task to finish, then mirrors a cancellation that came from another thread onto
that latch, aborts and awaits every task of the view — which is what makes it
the last owner of the page — runs the page's JavaScript disposal, and drops the
realm in a job of its own. Why a view ended is recorded nowhere: what the
embedder was told is whatever was reported before the end, and a release is the
token having been cancelled from outside. A report that ends the object — a
view's `StartupFailed`, a worker's `Failed` or `Closed` — goes through the
driver's `terminal` and the lifetime's terminal latch, so the first is the only
one sent. A panic is the one end that owes a report whatever was reported
before it, `Panicked`, through a panic latch of its own that the terminal latch
does not gate, and the payload rides the `JoinError` the set yields.

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
with its script, post to a context, stop a context) and receives events back.
The one other thing that crosses is a flag `bobcat-workers` sets when it traps,
which `bobcat-main` reads before each `Start`. Separating
them is the whole point of a worker: script that must not stop the thread that
owns the document. Because `QuickJS` binds a runtime to one thread, that
separation is also what makes "a worker cannot touch the document" structural
— there is no path from a worker realm to a `LynxDocument`, and no value of
either runtime can be named by the other.

Both threads open a realm through one constructor, `realm::open_realm`. It
creates the realm on that thread's runtime, enables module loading, and
installs the core every realm has under `bobcat-internal:host`: the
display-frame demand, the timer pair, the three `Future` members,
`fetchResource`, the two members `bobcat:module` is written over, and the two
`bobcat:diagnostics` is written over, `reportScriptError` and
`logScriptMessage`. They reach the view through the `HostOutbox` the caller
passes, whose token is the view's for an MTS realm and the worker's own for a
worker realm, so a synchronous wait in either ends with the realm that asked,
and a worker realm's diagnostics reach the host from its own thread, with no
message to the main-thread realm. The realm's other
host modules are a parameter of the same call: the document, stylesheet,
startup and `Worker` members for an MTS realm, `bobcat-internal:worker` for a
worker realm, and `bobcat-internal:native-modules` for both, which
`native_module::install` installs with the same one member in each. The
constructor names no realm kind. It is told two things: the key the realm's
display-frame demand is reported under, `None` for an MTS realm and the
worker's key for a worker realm, and the `ScriptSource` its `ScriptReported`
and `ConsoleMessage` carry, `Main` for
an MTS realm and, for a worker realm, the one its `WorkerStart` carries:
`Background` for the URL `bobcat:bts`, otherwise `Worker(WorkerId)`. Since
both runtimes register
every built-in module, these host modules are also what decides which
built-ins a realm can link.

### Realm construction and driving

There are three kinds of realm: a view's MTS realm on `bobcat-main`, the
view's BTS on `bobcat-workers`, and one plain `Worker` realm per `new Worker`,
also on `bobcat-workers`. All three are opened by `realm::open_realm`
(`realm/mod.rs`), driven by the one driver in `realm/owner.rs`, and report
their failures through the tables in `realm/policy.rs`. What differs between
them is what the opening call is passed and what the realm's owner — a `Page`
(`main/page.rs`) for the MTS realm, a `Worker` (`background/thread.rs`) for
the other two — adds to the driver.

| | MTS realm | BTS | plain `Worker` |
| --- | --- | --- | --- |
| Opened by | `Page::open_realm`, the view's first job | `Worker::boot`, the worker's first job, queued as its `Start` is served | `Worker::boot`, as for the BTS |
| Root module | `bobcat:boot`, which Rust generates: the page configuration and the screen as literals, the `Document`, `try { await import(<entry URL>) } catch`, then the BTS, the render and the first flush | `bobcat:bts` itself, the module at its URL, which imports `bobcat:worker` and `bobcat:timers` first | the script at its URL itself, with nothing written around it; it imports `bobcat:worker` and `bobcat:timers` itself when it uses them |
| Entry completed by | `load_entry`, from the answer `create_lynx_view` asked for | no entry of its own at boot: `bobcat:bts` imports the view's `background_entry` once `initialize` has arrived, as an ordinary import | `consume_messages`, from the answer to the request `createWorker` made |
| Core members on `bobcat-internal:host` | `requestScriptFrame`, `setTimer`/`clearTimer`, `waitFuture`/`takeFuture`/`settleFuture`, `fetchResource`, `resolveModuleUrl`/`loadModuleSync`, `reportScriptError`/`logScriptMessage` | the same | the same |
| Role host members | on `bobcat-internal:host`: the document, tree, attribute, readback, stylesheet (`preloadStyleSheet`, `adoptStyleSheet`), event-name, startup-string and `Worker` members | `bobcat-internal:worker`: `postWorkerMessage`, `closeWorker`, `workerName`, `backgroundEntry`, `pixelRatio`/`pixelWidth`/`pixelHeight` | the same members; `backgroundEntry` and the three screen members answer `undefined` |
| Startup strings | `initData`, `globalProps` and `initialProcessor`, one-shot members `bobcat:runtime` reads as it is evaluated | none: the `initialize` message carries `initData`, `updateData`, `processorName`, `cacheData` and `globalProps`, as the MTS realm processed them | none |
| `SystemInfo` | `ViewSources::screen`, written into the boot module as three number literals: a `bobcat:runtime` export and `lynx.SystemInfo` | the same screen, `BackgroundStart::screen` in its `WorkerStart`, read through the three screen members: a `bobcat:bts-runtime` export, `lynx.SystemInfo` and a global `SystemInfo` | no screen: a `bobcat:bts-runtime` it imports reports the runtime constants alone |
| `NativeModules` | `bobcat-internal:native-modules` (`invokeNativeModule`, `nativeModuleTable`) with an empty table; `NativeModules` is `undefined` | the same host module with the view's module table, `BackgroundStart::native_modules`, which `bobcat:bts-runtime` builds `NativeModules` from | the same host module with an empty table |
| `console` | a module binding: `bobcat:runtime` re-exports the `console` of `bobcat:diagnostics` | the global `console` `bobcat:worker` installs; `bobcat:bts-runtime` exports the same object | the global `console` `bobcat:worker` installs |
| Creates Workers | yes: `createWorker`, `sendWorkerMessage`, `terminateWorker`, and the `bobcat-internal` class over them | no | no |
| Frame demand key, `ScriptSource` | `None`, `Main` | the worker's key, `Background` | the worker's key, `Worker(WorkerId)` |

The native module transport is the same in all three. A call names the realm
that made it, and `LynxView::pump` answers it through the view's command FIFO
(`ToMain::ModuleCallback`) for the MTS realm and through the worker's inbox for
a worker. Only the BTS is given a table that names modules, so a plain
`Worker`'s `NativeModules`, if it imports `bobcat:bts-runtime`, is an empty
object. `pump` checks the module and method a call names against the view's
modules, not against the caller's table, so code in a plain `Worker` or the MTS
realm that imports the internal module `bobcat:native-modules` directly can
still call a module the view has.

**One built-in table.** Both runtimes are built by `esm.rs`'s `build_runtime`,
which registers the same twenty-one built-ins, `BUILTIN_MODULES`, and reserves
the two engine prefixes `bobcat:` and `bobcat-internal:` on the runtime. The
host modules a realm declares decide which built-ins it can link: importing a
host module the realm does not declare fails with a `ReferenceError`, and
importing a member its host module lacks fails at link with a `SyntaxError`. A
name under either prefix that no runtime registered and no realm declared fails
where it was asked for — an `import`, or a `require` through `loadModuleSync` —
with a `ReferenceError` (`module '<name>' is not preloaded`), in the bridge's
own loader, and is never sent to the fetcher. `bobcat-internal`, the `Worker`
class, has no colon and is covered by neither prefix; both runtimes register
it, so it is never fetched either. The prefixes are the module loader's check
alone: the startup requests `create_lynx_view` makes, stylesheets, fonts and
fetches are not checked, and `createWorker` still asks the host for a
`new Worker` URL under them, which the realm then loads through the loader
as the worker's root module.

**The driver.** An owner supplies the driver a `RealmOwner` impl: where its
`Lifetime`, its runtime, its realm and its `HostOutbox` are, where its reports
go (an `EngineEvent` to the host for a page, a `WorkerPayload` to the creating
realm for a worker), which table its failures are read from, and the hooks
below. The functions in `realm/owner.rs` are the rest, written once:

- `spawn` starts a task of the owner under a guard that ends the owner if the
  task unwinds;
- `enter` queues one job and answers with what its operation returned, and
  `enter_now` is that job's body: it returns `None` for an owner that has
  ended, borrows the shared runtime (`None` for a runtime that was never
  built), borrows the realm (`None` before it opened or after its release),
  runs the operation, and then the epilogue;
- `end` sets the lifetime's latch once, and the call that set it runs the
  owner's `on_end` hook: the `BeginFrame` acknowledgement for a page, nothing
  for a worker;
- `terminal` sends an event that ends the owner through the lifetime's
  terminal latch and then ends it, and `trapped` sends the table's Panic row
  through the panic latch and then ends it;
- `run_owner` is the owner task's tail: the lifetime's wait, `end`, the reap
  of every task, the owner's `before_release` hook — the page's JavaScript
  disposal, nothing for a worker — and a job that releases the realm;
- `after_end` is a job against the realm after the end, with no latch and no
  epilogue, which `before_release` and the release run as.

`Settles`, which `serve_clock`, the unwind guard and `run_job` in
`lifetime.rs` are written over, is one blanket impl over every `RealmOwner`.

The epilogue's steps and their order are the contract, for both owners:

1. nothing, for an owner that has ended — by its operation, or by a task
   that ran during a synchronous wait inside it;
2. the due timers, each callback that threw reported under Timer: on a page
   the realm's timers and then the end of the batch they ran, which runs the
   collection their removals may have made due; on a worker none once its
   script has called `close()`;
3. `after_timers`: on a page the commit — skipped while a listed sheet is
   outstanding or has failed — and the posted content-visibility and
   `<image>` deliveries; on a worker a `close()`, which ends it with `Closed`
   through `terminal`;
4. nothing more, for an owner that step ended;
5. the boot report, until the root module has settled: a root module that
   finished is marked and `on_booted` runs, which sends `ScriptFinished` on a
   page, while a worker's mark releases the posts its consumer held; a root
   module that rejected is marked, and reported under the page's
   `BOOT_REJECTION` scene on a page — a worker's `BOOT_REJECTION` names none,
   because the entry the rejection happened in has already reported it;
6. nothing more, for an owner that report ended;
7. `after_boot`: on a page the `BeginFrame` acknowledgement, after both the
   commit and the boot report;
8. the module requests the operation left, each asked of the host through the
   owner's `HostOutbox` and spawned as a `load_module`, except the one
   `entry_name` names — the MTS entry, which `load_entry` answers, or a plain
   `Worker`'s script, which `consume_messages` answers;
9. the futures a `.then` asked the realm to settle, each spawned as a
   `settle_future`;
10. `after_settles`: on a page one `load_font_face` per `@font-face` rule the
    sheets mounted by the operation declared;
11. the next timer deadline, republished only when it moved;
12. the checkpoint generation, last, so it names the generation this entry ran
    the shared job queue up to, which is what `serve_clock` compares a bump
    against to tell this owner's entries from a sibling's.

The only collection the engine forces is the MTS realm's: a batch of document
operations that crossed `REMOVALS_PER_COLLECTION` removals ends with one, in
the call that ran the batch — the timer batch of step 2 is one such call, and
an event dispatch or a page update is another. The epilogue has no collection
step of its own.

`load_module` waits for its answer outside any job and reads it with
`module_answer`, the one reading of an answer to a module request, which
`load_entry` and a plain `Worker`'s `consume_messages` use too: a script is its
response URL and its source, a failed load is the fetcher's own error, and an
answer of another kind is a `Script` error `the fetcher returned a <kind> for
<url>`. A completion the fetcher dropped without answering is
`unanswered_source()`'s failure, through `await_source`. It then enters the
realm and completes the module under the name the import asked for: from the
response URL, which becomes the module's `import.meta.url` and the base of its
own imports, or with `module '<url>': <error>`, which rejects the import in
the realm. `ScriptEngine::complete_module` replaces a NUL in that text with
U+FFFD and fails a response URL that contains one, because the bridge would
refuse either without completing the module. `settle_future` waits for its
operation and enters the realm to hand the outcome over. What either entry
returns is reported under Module or Future.

A panic that unwinds an owner's own task — `serve_view` or `serve_worker` — or
a whole engine thread is not the driver's to report, because the `Rc` of the
owner that task held is dropped with it. The thread reports it from its own
table of who to tell: `finish_view` and, on Wasm, the panic hook on
`bobcat-main`; `finish_worker_task`, `report_thread_trap` and, on Wasm, the
panic hook on `bobcat-workers`. `report_thread_trap` is the whole thread
trapping: it sets the flag `bobcat-main` reads before each `Start` and sends
`Failed` to the creator of every worker still live there. Each of these builds
its event with the Panic row of its realm kind's table, the row `trapped`
reports through.

**Failure reports.** What a failure is reported as is looked up by the scene it
happened in, in the owner's realm kind's table. `policy::report` reads the
row: a row that ends the owner reports through `terminal`, and any other is
sent as it is. The Panic row of each table is a function of its own, and so is
the MTS table's Disposal row, so nothing that takes a scene can produce
either. A row's prefix is written before the failure's message as
`<prefix>: <message>`; where it is "none" the caller has already named what
failed.

The MTS table, whose events go to the view's host:

| Scene | Event | Ends the view | Prefix | Reported by |
| --- | --- | --- | --- | --- |
| Open | `StartupFailed` | yes | none | `Page::open_realm`: a runtime that was never built, the realm's construction (`opening the MTS realm`), boot's own module (`booting the MTS entry`); `load_entry`, for an entry the fetcher could not load, one it answered with something other than a script, and naming the entry (`booting the MTS entry`), each a `LynxViewError` that `StartupFailed` carries as it is, through `terminal` rather than through the row; the epilogue, for a rejection of boot's own module, as `booting the MTS entry` |
| Boot | `ScriptRunError` | no | none | `load_entry`: the entry's evaluation, a module it imports included |
| Module | `ScriptRunError` | no | `loading an imported module` | `load_module` |
| Future | `ScriptRunError` | no | `settling a future` | `settle_future` |
| Timer | `TimerFailed` | no | none | the epilogue |
| Listener | `ListenerFailed` | no | none | an input event (`ToMain::DispatchEvent`), a batch of `<image>` outcomes, what a worker said (`consume_worker_events`) |
| Frame | `ScriptRunError` | no | none | `ToMain::Vsync` |
| HostCall | `ScriptRunError` | no | none | `ToMain::PageUpdate`, `ToMain::ModuleCallback` |
| Disposal | `ListenerFailed` | no: the view has already ended | none | the page's `before_release`, the JavaScript disposal |
| Panic | `Panicked`, through `EngineEvent::from_panic` | yes, through the panic latch | `the Lynx main thread panicked` | `trapped`, `finish_view`, the Wasm panic hook |

`EngineEvent::is_fatal` names exactly `StartupFailed` and `Panicked`, which
`LynxView::pump` ends a view on, and a unit test pins that an MTS row ends the
view exactly when its event is fatal. `processData` and render hook failures
are in neither table: `main-thread-runtime.ts` catches them and reports a
`ScriptReported` diagnostic, and boot goes on.

The worker table, whose events go to the realm that created the worker:

| Scene | Payload | Ends the worker | Prefix | Reported by |
| --- | --- | --- | --- | --- |
| Open | `Failed` | yes | none | `Worker::boot`: a runtime that was never built, a realm that could not be opened; `consume_messages`, for a script the fetcher could not load or answered with something other than a script, as `loading the worker's script` |
| Boot | `Errored` | no | `running the worker's script` | `Worker::boot`, for the load of the root module; `complete_script`, for a plain `Worker`'s script |
| Module | `Errored` | no | `loading an imported worker module` | `load_module` |
| Future | `Errored` | no | `settling a worker's future` | `settle_future` |
| Timer | `Errored` | no | `running a worker's timer callback` | the epilogue |
| Listener | `Errored` | no | `delivering a message to a worker` | a posted message |
| Frame | `Errored` | no | `running animation callbacks` | a vsync, through `bobcat:animation-frame` |
| HostCall | `Errored` | no | `running a native module callback` | a native module's answer |
| Panic | `Failed` | yes, through the panic latch | `the worker thread panicked` | `trapped`, `finish_worker_task`, `report_thread_trap`, the Wasm panic hook |

The MTS realm that created the worker reports an `Errored` to the host as
`WorkerThrew` and a `Failed` as `WorkerEnded` (see the worker errors below),
and neither is fatal to the view. `close()` is not a failure: step 3 of the
epilogue reports `Closed` through `terminal`, so it shares the terminal latch
with `Failed`, and the first of the two is the one sent.

The epilogue reports the rejection of a root module on the MTS realm alone,
as Open, the page's `BOOT_REJECTION`, because the root modules are different
code. The MTS boot module is the engine's: it catches what the entry's
evaluation throws, and `main-thread-runtime.ts` catches what `processData` and
the render hooks throw, so what rejects it is `bobcat:runtime` reading page
data that is not JSON, the document's construction, connecting the BTS, or
boot's own flush — something the engine could not make ready, which is what
Open means in both tables. A worker's root module is the module at its URL,
and a worker's `BOOT_REJECTION` names no scene. Only the host holds that
load's promise, so a checkpoint of the realm reports its rejection as it
reports every rejection nothing handles: the checkpoint that ends the entry
the load settled in reports it under that entry's row — Boot for the boot job
and the script's completion, Module for a module the script imports, Timer
for a timer — or drops it with the other leftovers of the one failure that
entry reported. The epilogue reads the load only to learn that it has
settled, so a failure of the root module is reported once, and the worker
goes on running, as HTML's "run a worker" leaves it. The BTS's root module,
`bobcat:bts`, imports registered modules only; its entry is imported later,
by the loader `bobcat:bts` hands `bobcat:bts-runtime`, once `initialize` has
arrived, and what the entry throws is reported through the worker global's
`reportError`, not as a rejection of the root module.

A listed stylesheet has no row of its own. The listed sheets are settled by
boot's first `__FlushElementTree`, one at a time in listed order, and nothing
is committed before them. A sheet that failed to load, or that the fetcher
answered with something else, makes that flush throw `loading stylesheet
<url>: <reason>`, which rejects boot's own module: the Open row, a
`StartupFailed(LynxViewError::Script(..))` naming the sheet, and the view ends
without a `ScriptFinished`. A card that settles the listed sheets first — an
`adoptStyleSheet` or a `__FlushElementTree` of its own inside the entry's
evaluation — sees the failure thrown to it, reported under the row of the
entry it ran in if it does not catch it; `DocumentSlot` keeps that first
failure and throws it again from boot's own flush, which fails the boot the
same way.

**Not part of this construction.**

- The worker runtime is never made to collect. The forced collection above is
  the MTS realm's, driven by document removals; the BTS and every plain
  `Worker` are collected only on QuickJS's own allocation pressure.
- A bundle's paths map to URLs as before, and the two realms still differ: the
  MTS names the page's own chunks under its entry's URL (`bobcat:section-url`),
  and the BTS resolves a path of a registered container beside that
  container's template URL (`bobcat:lynx-modules`).
- No worker realm creates a Worker: the `Worker` members are the MTS realm's
  alone.
- The MTS realm has the native module transport and no API over it:
  `NativeModules` there is `undefined` (see `docs/tracking/deviations.md`).

### Workers, modules and the boot module

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
rejected.

A message crosses as a **structured clone**: the host-function boundary
serializes the value with the engine's own serializer (`JS_WriteObject` with
object references, never bytecode and never shared memory), the bytes cross the
existing channel inside a `HostValue`, and the receiving realm rebuilds the
value with `JS_ReadObject`. Preserved: `undefined`, `NaN` and `-0`, `Date`,
`BigInt`, `ArrayBuffer` and typed arrays, cycles and shared references (two
properties naming one object still name one object on the other side), and the
`Number`/`String`/`Boolean`/`BigInt` wrapper objects. Refused, because this
first version does not extend the engine's serializer: functions, `Symbol`s,
`Map`, `Set`, `RegExp`, `Error`, `DataView`, and accessor properties. `toJSON`
is never consulted: structured clone has no such hook, so an object carrying
one is refused for the method it holds rather than replaced by its result. A
refusal is thrown synchronously **at the `postMessage`/`dispatchEvent` call**,
in the realm that wrote the value — a `TypeError` for an unsupported object
class or a non-value property, an `InternalError` for an unsupported tag — and
the host is never dispatched to at all, so nothing partial is sent.

Transfer lists, credentials options, and worker-local
`onerror` remain unsupported. `messageerror` is absent because the reader
cannot fail on what the same build's writer produced.

```text
main realm: new Worker(url)
  ├── WorkerStart { key, name, url (joined to __Card__),
  │                 script: oneshot receiver (None for an engine name),
  │                 source: Background (bobcat:bts) or Worker(id),
  │                 messages: mpsc receiver, events: this view's sender }
  │        ────────────────────────────────▶ bobcat-workers: one task per worker
  └── ViewNotice::RequestSource ──▶ LynxView::pump ──▶ request_source
      (not for an engine name)       │ SourceRequest::Module(url)
                                     └── SourceCompletion answers the oneshot
                                         that already rode inside the Start
main realm: postMessage / terminate ───────────────▶ that worker's own task
main realm: Worker message/error handler ◀── WorkerEvent { key, payload }
```

The script URL is resolved in Rust, before anything is started: `createWorker`
joins every specifier by URL rules — not import-specifier rules, so
`worker.js` and `?v=2` are relative URLs, and an absolute URL such as
`bobcat:bts` joins to itself — to the creating view's entry response URL,
which the realm holds as `__Card__` and passes as the third argument; Rust
does not keep it. A URL that does not resolve allocates no key, sends no
`Start` and requests nothing, and `new Worker` throws HTML's synchronous
`SyntaxError`. There is one kind of worker. The BTS is the dedicated worker
whose URL is `bobcat:bts`, and the URL alone decides the two things that
differ between workers: only a URL outside `ENGINE_MODULE_PREFIXES` is
requested from the host, and only `bobcat:bts` is named
`ScriptSource::Background`. No `Start` carries the view's data: the MTS realm
posts it to the BTS in the `initialize` message.
Fetching and UTF-8 validation remain fetcher policy. Multiple worker requests
are preserved without coalescing. The `WorkerStart` is sent before the host is
asked to fetch, so messages posted during loading queue against an existing
key. The worker's realm opens as its `Start` is served, with the view's realm
as the model, and loads the module at the worker's URL as its root module,
the way `import(<URL>)` loads one: the engine writes nothing around the
script and installs no global scope before it, and the request that load
makes is never sent again. The completion answers the worker's own message consumer
directly: it needs no main-thread turn and cannot be held behind a long
main-thread script. The consumer completes the module under the request URL,
from the response URL, and holds what is posted until the root module has
finished, which is what HTML does. A runtime that never came up fails the
worker at its `Start`, without waiting for the answer. A worker told to
terminate before its script arrives never runs it, because the consumer's
wait for the script is a `biased` select with the message channel first. The
consumer starts beside the worker's first job, the one that opens its realm,
rather than after it: that job can be queued behind another realm's job
parked on a synchronous wait, and a `Terminate` read meanwhile ends the
worker and cancels its fetch at once; the job then opens nothing. A URL that
is an engine name has no answer to wait for: the host is never asked for it,
and the realm's own loader loads a registered name such as `bobcat:bts` or
`bobcat:timers`, or refuses any other with a `ReferenceError`, which the
worker reports as `WorkerThrew` and keeps running. The worker's token is
independent of the view, so its cancellation cannot race ahead of JS
disposal.

A worker script imports its global scope itself. `bobcat:bts` begins with
`import "bobcat:worker"; import "bobcat:timers";`, and a plain worker script
that wants `self`, `postMessage`, `onmessage`, `close`, `name` or `console`
writes `import "bobcat:worker";`, and one that wants `setTimeout` and its
companions `import "bobcat:timers";`. A script that uses them without the
import throws a `ReferenceError`, reported as `WorkerThrew` like any other
throw; nothing guards against that. The engine itself constructs only the
BTS; a plain `Worker` comes from MTS code that imports `bobcat-internal`, or
from tests. What is posted is delivered through `bobcat:worker`, so a realm
in which that module has not run has nothing that receives a message: the
post is dropped and nothing is reported. The test is the module having run,
which the host learns from its read of `workerName`, the module's last
statement, and not the realm having an instance of it: a graph that failed
to load, or is still loading, leaves its modules compiled but never linked,
and the namespace of such a module cannot be read.

Worker keys are allocated once per group on main and never reused. A worker's
whole state is its own task; `bobcat-main` keeps two things per worker. One is
the sending end of its message channel, and only while that worker runs — a
worker that closed itself or failed is forgotten where the realm learns of it,
when that event is dispatched. The other is its `ScriptSource` (`Background`
for `bobcat:bts`, `Worker(WorkerId)` for any other URL), recorded when the key
is allocated, before the `Start` carrying the same source is sent, and removed
at `terminate()` or at that same dispatch; a worker that fails before it is
started, and so never had a channel here, has one too. MTS keeps
`WeakRef<Worker>` values for event routing; a JS `FinalizationRegistry`
releases an unreachable Worker's sending handle.
A reachable Worker survives collection. Explicit `terminate()` uses the same
release path and unregisters its finalizer. Both stop the context between tasks
and discard queued messages without interrupting synchronous JavaScript.
Releasing the MTS realm closes its remaining senders naturally. Host callbacks
reference their channel owner weakly, so finalizers queued during realm release
cannot retain it. Worker entry/import completions use the Worker's independent
token and become cancelled when that Worker ends.

Worker errors still produce a nonfatal host event, one of two. A worker's
`Errored` — something its realm ran threw, whichever entry it was, and the
worker still runs — is `EngineEvent::WorkerThrew`; its `Failed` — its script
could not be loaded, its realm could not be built, or `bobcat-workers`
trapped, before or after it was started — is `EngineEvent::WorkerEnded`. A
failure of a worker's root module — a throw at its top level, a dependency
that could not be loaded or that threw, a rejected top-level await — is one
`Errored`, reported by the entry it happened in (the boot job, the completion
of the script or of a module it imports, a timer); the worker's own read of
the root module's load only learns that the load has settled. The two events
both carry the `ScriptSource` recorded for the key, which `dispatch_worker_event`
reads before it forgets an ended worker, and both are reported before the
realm's JS dispatches the `Worker`'s `error` event, so `preventDefault()` there
does not suppress them. A key without a source reports neither: a worker the
script stopped with `terminate()`, or whose `Worker` object was collected, is
reported to no one, as the JS dispatch drops its events too, and a `close()`
is no event at all. Delivering a worker's own end removes its source too, so
a worker reports its end once: a trap that reaches a worker after its
`Failed` or `Closed` was delivered is reported to no one. The source table is
kept apart from `live`, so a worker created after the trap, which never
entered `live`, still reports its `WorkerEnded`. A private JS close notification lets MTS disposal finish when
BTS already closed or failed.
MTS keeps its Worker reference after that Worker ends; a post to an ended
Worker is dropped by the host, as a browser drops `postMessage` to a terminated
worker, and nothing accumulates in a queue for it.
An application listener that throws during a DOM event dispatched from Rust
reports `ListenerFailed`. A JS `EventTarget` listener — a Context event, a
`Worker` `message` or `error` event, an engine event — follows the DOM's
inner-invoke rule instead: the throw is reported and the walk continues with
the next listener, through `lynx.reportError` and the host's
`reportScriptError` (a nonfatal `ScriptReported` from `ScriptSource::Main`) in
the MTS realm, and through the worker global's `reportError`, hence the parent
`Worker`'s `error` event and a nonfatal `WorkerThrew`, in a worker realm. The
BTS treats an animation-frame, `queueMicrotask` or `lynx.fetchBundle`
callback that throws as the same uncaught exception; its `lynx.reportError`
is a diagnostic instead, a `ScriptReported` from `ScriptSource::Background`
sent to the host by the BTS realm itself.

Each MTS boot starts one BTS Worker named `lynx-bg` once its entry import has
settled, whether the entry succeeded or threw.
Boot constructs it through the same `bobcat-internal` class, using the engine
URL `bobcat:bts`, which is what tells the BTS apart. All workers use the same
protocol, and each one's root module is the module at its URL. BTS `lynx` is an ESM export from
`bobcat:bts-runtime`; neither MTS nor BTS sets `globalThis.lynx`. A BTS
application has the bindings it needs, `lynx` included, as imports of
`bobcat:bts-runtime`, without a dependency back to the bootstrap that starts
it. A card body — a compiled bundle's body, or an XML page's background-thread
script — has them from the `BTS_CHUNK_PREAMBLE` `bobcat-source` prefixed it
with, and must not import any of them itself (a second binding is a
`SyntaxError`); a raw entry imports them explicitly.
`bobcat:bts` is a registered module (`packages/bobcat-element/src/bts.ts`)
that is the BTS realm's root module, as every worker's root module is the
module at its URL, so nothing is fetched to start the BTS, and its `Start`
is built like any other worker's. It imports `bobcat:worker` and
`bobcat:timers` first, which is where the BTS's global scope and timers come
from. The bootstrap passes a loader,
`async ({ entry }) => { if (entry !== undefined) await import(entry); }`, to
its JS initializer and returns, without a top-level `await`, so the root
module finishes and the first message is delivered. That message is the
`initialize` the MTS realm's `__BobcatConnectBackground` posts: the page's
data, the view's `background_entry` (which boot's source names as a JSON
literal, `undefined` when the view named none), the MTS realm's own
`SystemInfo`, and the view's native module table as the record the MTS
startup member `nativeModuleTable` answered. It initializes BTS inputs,
`SystemInfo` and `NativeModules` included, before the loader runs; later
messages wait on its Promise. XML takes exactly this path. Without an entry,
the same initialization message supplies the Context and app/native-app
environment, then BTS acknowledges it.

Workers use the same asynchronous ESM loader as main. Each discovered module
gets a source completion on the view's existing host channel; its final response
URL becomes the base for dependencies. A per-worker boot watch gates posted
messages until the worker's root module, the module at its URL, settles. The BTS runtime separately holds
its messages on the application import Promise; completions and timers continue.
Cancellation follows the worker's own token; source completions never travel
through the MTS realm. Handled import failures leave the worker usable; a BTS
startup failure reaches the MTS failure binding. ReactLynx compiled-module and
lazy-bundle execution remain a later layer over this resource transport; see
[Worker resource loading](worker-resources-runtime.md) for the actual caller
contract. There is no separate source-text request/callback protocol. The runtime
cost remains one worker realm per view, with no additional OS thread or runtime.

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
structured-clone copy, for early and connected sends alike — so what a Context
event carries is whatever that transport preserves, and a value it refuses
throws at the `dispatchEvent` call. That pre-connection queue is only for
messages the MTS entry itself produces, before boot constructs the Worker; it
is not a holding area for anything else. The worker's task queues what
is posted until its entry has evaluated. Worker release, source cancellation
and `WorkerThrew` / `WorkerEnded` reporting apply to BTS too, from
`ScriptSource::Background`. `ScriptFinished` means MTS boot finished: the
entry module evaluated, its top-level await settled, and its first flush
committed. The BTS Worker's state — still importing its entry, its
entry threw, or it ended — is no part of that, so a BTS entry whose top-level
await never settles does not keep the view from becoming ready.
A BTS entry that throws is reported like any worker script: the worker
realm's `reportError` surfaces it at the `Worker`'s `error` event and as a
nonfatal `WorkerThrew`, and BTS stays up and still takes messages.
No BTS failure ends the view.
`LynxView::pump` records readiness before returning `ScriptFinished`, and
`is_ready()` exposes that state. Host global events require that observed MTS
boot and return
`EngineError::NotReady` otherwise, without buffering them.

The BTS runtime exposes stable `lynx.getApp()` and `lynx.getNativeApp()`
objects. MTS `__OnLifecycleEvent(data)` sends the existing Context event;
the BTS listener calls the current `app.OnLifecycleEvent(data)` with the app
as receiver. Separate runtime Worker messages carry `publishEvent`,
`publicComponentEvent`, `callDestroyLifetimeFun` and `callLepusMethod`, so
these calls do not become application Context events. Context and runtime
messages share the MTS queue before Worker connection.

String `__AddEvent` handlers, including an empty string, now publish an event
snapshot to BTS. The snapshot is the event's own fields with its two stop
methods destructured out and its element handles replaced by values; the copy
itself is the transport's, taken at send time. Target identities contain
`dataset`, `id` and `uid`; no
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
undefined; null remains null. Failed calls, rejected results and values the
transport refuses report through the existing Worker error path without
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
queue. The realm checkpoint and the QuickJS bridge's call path are unchanged;
the Worker transport is the structured clone described above, so undefined
members, nonfinite numbers, negative zero, BigInts and cyclic objects all
survive a call and its reply. No custom value codec or extra deep clone sits on
top of it: a value the serializer refuses throws at the call, and transfer
lists remain outside this endpoint's scope.

An explicit JS `lynx.getEngine().dispatchEvent({type: "__DestroyLifetime"})`
starts MTS's disposal Promise: post `dispose`, await the BTS `disposed` reply,
then terminate the Worker. BTS calls its current `app.callDestroyLifetimeFun`,
reports a throw and replies after the ordinary Promise boundary. MTS teardown
uses this same Promise, so repeated notifications do not repeat app cleanup.
The page owner evaluates the ordinary `bobcat:dispose` ESM and continues routing
Worker events until its TLA completes before releasing MTS. See
`destruction-runtime.md` for the Worker and object finalization boundaries.

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
members, the two event-name members, the two timer members, and the three worker
members — then preloads three kinds of ESM source: the core-owned
`bobcat:runtime` named compatibility exports, the embedded `bobcat:element`
named Element-PAPI exports, and the fetched entry under the URL boot imports
it by, answered from its resolved URL.
`bobcat:element`
imports its native operations directly; nothing is installed as
`globalThis.bobcat`. The entry is completed as the fetcher answered it: a
card's entry carries its runtime and Element-PAPI import declarations because
`bobcat-source` wrote them in front of its body. Event delivery travels back
through the loaded `bobcat:element` namespace's `__BobcatDispatchEvent` export, once per
dispatch, carrying the whole event path as two comma-joined id strings and
everything else as primitives: whether the event bubbles — which decides how
much of that path the bind pass runs on, and whether the `global-bindEvent`
pass runs at all — the event's `timestamp`, and a numeric detail kind followed
by the numbers that kind spends (a position and an optional wheel delta plus
four numbers per touch point for a routed input event; an intrinsic size for an
`<image>`'s `load`; none at all for its `error`).

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

`bobcat:module` is the synchronous one. Both realm kinds can
`import { createRequire } from "bobcat:module"` and call
`createRequire(import.meta.url)` for Node's `require`. It is a source module in
`packages/bobcat-element`, and the algorithm — cache, `module` object, cycles,
eviction, `require.resolve` — is JavaScript in it. Two host members on
`bobcat-internal:host` carry what is not: `resolveModuleUrl(base, specifier)`,
the same normalizer an `import` resolves through, and
`loadModuleSync(url, parameters)`, which resolves `url` against the view's
`ViewSources::base_url` by URL rules — in every realm of the view, a worker's
included, and an absolute URL resolves to itself — requests the result as the
same `SourceRequest::Module` and answers the source compiled — the wrapper function
of a CommonJS file, or the parsed value of a JSON one — so source text never
becomes a value in the realm. That load parks the job it runs in on the answer
the way stylesheet adoption does: the engine thread's tasks keep running, no
other job does, and no promise job runs. The wait's other arm is the requesting
realm's cancellation token: a view's is written by the embedder's release from
the embedder's own thread, a worker's by the in-band `Terminate` its message
consumer reads while the job is parked. A response URL whose path ends in
`.json` is parsed as JSON; everything else is compiled as CommonJS in Node's
wrapper, named by the response URL. The compile happens before the compiled
script is evaluated, which is what keeps the borrowed source buffers safe from
a file whose text closes the wrapper early and loads again from there. The
CommonJS cache is per realm and is not the ESM module map. Every source module
also carries `import.meta.url`: the response URL where the realm fetched one,
the registered name for a built-in, and the source name for a module evaluated
directly.

`bobcat:future` is one host-backed operation a realm can read either way. A
`Future` is a number — the id the host's per-realm table registered a Rust
future under — because only primitives and structured clones cross the
boundary. `wait(timeout?)` is the one synchronous park beside stylesheet
adoption: it parks the job it runs in, so the engine thread's tasks keep
running, no other job does, and no promise job runs. The wait's first and
biased arm is the requesting realm's cancellation token — a view's written by
the embedder's release from the embedder's own thread, a worker's by the
in-band `Terminate` its message consumer reads while the job is parked — and
the optional deadline is behind it. A deadline that passes throws a
`TimeoutError` and cancels nothing: the operation goes on, and the same Future
still answers a later read. `then` is the asynchronous way: it converts the
Future into one Promise, the owner's epilogue spawns a task that awaits the
operation, and that task enters the realm to deliver what it settled to,
rejecting with an `Error` carrying the host's reason. A `wait` after that
conversion is a `TypeError`, because the delivery is a job and a job cannot run
inside another job's wait. The class is the `packages/bobcat-element` source
module `future.ts`, over three host members — `waitFuture(id, timeoutMs)`,
`takeFuture(id)` and `settleFuture(id)`. Both realm kinds have all three. The
production operation registered in it is `fetchResource(url)`'s plain host
fetch — the member `lynx.fetchBundle`'s `{wait, then}` handle is written over,
one Future per fetch, except for a URL the fetcher's `fetch_probe` says this
view already fetched, which answers `true` and registers nothing; a test-only
`testFuture` producer exercises the table itself.

The bridge keeps built-in sources on the shared runtime and entry/imported
sources on each realm. A missing module creates one `SourceRequest::Module` per
normalized URL in that realm. The boundary's epilogue spawns one task per
queued request, and `LynxView::pump` forwards each through
`ResourceFetcher::request_source`. An import's IO never blocks a job. The
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
The boot promise tracks only MTS evaluation, and only the engine's own code
rejects it: the entry's import is caught inside boot. Its rejection sends
`StartupFailed`; `ScriptFinished` is published after it fulfills and boot's
first flush commits.
Imports started after boot use the same loading path. Dropping a view cancels
its completion handles and releases its suspended continuations.

The final `bobcat:boot` module imports the lifecycle helpers from
`bobcat:runtime`, `Document` and `__FlushElementTree` from `bobcat:element`,
and `bobcat:timers` for its effect. The runtime parses the initial JSON before entry execution. The
generated boot body has this order:

```js
const config = {
  defaultDisplayLinear: true,
  defaultOverflowVisible: true,
  enableCssSelector: true,
  enableJSDataProcessor: false,
};
export const document = new Document(config);
__BobcatInitializeMTS({
  enableJSDataProcessor: config.enableJSDataProcessor,
  systemInfo: screenMetrics,
});
let data = lynx.__initData;
// the entry's URL, as the view named it; what the entry throws is raised
// again as a rejection nothing handles, and boot goes on
try { await import("app:///main.js"); } catch (error) { void Promise.reject(error); }
const { Worker } = await import("bobcat-internal");
data = __BobcatProcessInitData(data);
__BobcatConnectBackground(new Worker("bobcat:bts", { name: "lynx-bg" }), data);
__BobcatRenderPage(data);
await Promise.resolve().then(() => __FlushElementTree());
```

The screen's three numbers, the page configuration's four switches and the
entry's URL are written into it as literals — facts Rust owns, passed as
primitives, with no JSON the realm parses and hands back. `new Document(config)` builds the
document and mounts nothing: the first `__FlushElementTree` mounts the author
stylesheets, in listed order, before it styles the document. The entry's `import` asks the fetcher for
nothing: a task of the view completes that name from the answer
`create_lynx_view` already asked for, answered from its response URL. That
task calls `__BobcatInitEntry` with the response URL before it completes the
module, so `__Card__` is that URL before the entry's body runs, and a `new Worker` URL
resolves against it — the realm passes it to `createWorker`, and Rust joins
the two by URL rules. An
entry that could not be loaded is never completed: `load_entry` fails the
boot with `StartupFailed` before any of it runs, and the import is released
with the realm.

The retained argument survives entry initialization replacing `lynx.__initData`.
Processing, the BTS snapshot and MTS render run synchronously. Boot then awaits
a flush queued with `Promise.resolve().then`, preserving the ordinary microtask
boundary; it does not drain Promise jobs between lifecycle hooks.
The first Worker message initializes BTS data before its entry imports. JS holds
later messages on that import's Promise and delivers them in order once it
settles, success or failure; import
failure reports through the same Worker channel. Host updates require observed
MTS boot, with no caching or replay before it, and are accepted while the BTS
entry still loads. See
[data lifecycle](data-lifecycle-runtime.md) for the inputs and readiness contract.

The global `renderPage` function remains a compatibility path, not a boot
requirement. An entry may instead register its renderer on the stable,
realm-local EventTarget returned by `lynx.getEngine()`. Rust evaluates one boot
module; it does not issue a second native lifecycle call after evaluating the
entry.

The engine EventTarget retains JavaScript listeners and receives render, update,
component-removal and global-props lifecycle events. The remaining MTS `getCoreContext`
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

The document belongs to a *realm*, and the realm creates it, out of a
configuration it holds. `bobcat:element` exports `class Document`; the boot
module is written with the view's four page switches as boolean literals and
constructs one over that record; and that constructor calls
`createDocument(defaultDisplayLinear, defaultOverflowVisible,
enableCssSelector, enableJSDataProcessor)`, which reads the four
`HostValue::Boolean` arguments — Rust genuinely needs these fields — and builds
a `LynxDocument` out of them plus the `DocumentIngredients` that never reached
the realm: the create-time viewport, the validated `dom::TextContext` and the
group's `StylePool`. It never waits and mounts no author stylesheet; the first
`__FlushElementTree` mounts those, in listed order. The construction runs
under a catch, because this member runs the UA cascade behind one call: a
panic in it fails the boot naming the phase that panicked, where any other
host member's panic ends the view as `Panicked`. A missing or
non-boolean switch and a realm that already has a document both throw, and the
throw rejects the boot module's own `new Document(config)`, which fails the
boot and ends the view — so nothing asks again, and the embedder is told
`StartupFailed(LynxViewError::Script(..))` naming the reason.

The document then lives exactly as long as the realm. The boot module's
exported binding holds the object, nothing in the realm releases it, and there
is no release member, no registry over the `Document`, and no answer a host
member gives without a document. That is deliberately not the path elements
take: cards genuinely unroot handles, and a collection every 32 removals frees
what they named.

Release is the view's task ending. Dropping the `LynxView` closes its command
channel; the task reaps ordinary view work, awaits MTS JS disposal, then drops
the `MainThreadRuntime`, whose fields drop
in declaration order — the `engine` field first, which holds the context's
`Rc`, so the realm is freed with it; freeing it takes the host functions the
realm held and their clones of the
`Rc<RefCell<DocumentSlot>>` with it, and the runtime's own `slot` handle drops
after that, which is when the `LynxDocument` drops. JavaScript goes first, then the Rust
object it named; the field order and its comment are the whole mechanism, and
no `Drop` impl stands behind them. Remaining Worker channels close with MTS,
and the frames watch closes with the same return; an attached painter keeps
showing the last frame it drew.

**There is no window in which a view has a task but no document.** Opening the
realm is the first job of the view, queued before any of its tasks is spawned
and before anything has been fetched, and the boot module's first statement is
what creates the document — so a command that arrived earlier is a job queued
behind that one and finds a document when it runs. Nothing is buffered, replayed
or dropped for want of one. The painter's metrics are not a command at all:
they are observed state on a watch the document reads for itself, so a
`Document` is already at the painter's size if one has attached.

**A flush waits for the listed sheets, then for the binding.** Before anything
is styled, the first `__FlushElementTree` settles the view's listed author
sheets, as [Startup boundary](#startup-boundary) describes: one park per
sheet whose answer has not arrived, in listed order, and a throw naming the
sheet if one failed. Until then `commit_if_dirty` skips, so no frame is
committed without them. The sheets come before the binding wait below, so
their IO and the painter's construction overlap and the frame held for the
binding already carries them.

**The painter's metrics bind the view, and a flush waits for the binding.**
`ViewSeat` carries a `watch<Option<Viewport>>`, `None` until a painter writes
it; `Painter::attach` writes it, an attached `Painter::resize` writes it again,
and `Painter::detach` leaves it alone. It is a watch rather than a command for
a structural reason: an unbound `__FlushElementTree` parks the *job* it runs
in, and while a job is parked no other job runs — so a command carrying the
metrics could never be applied, and the wait polls the watch directly instead.

Until the first write the document works at the create-time viewport:
`createDocument` reads the watch and builds at whichever of the two it finds,
every epilogue's `commit_if_dirty` reads it again and adopts it, and readbacks
before the binding answer at the create-time size. What a commit made before
the binding does **not** do is publish: the frame is held in the realm's
`DocumentSlot`, newest only, because a painter composes at its own size and has
no way to tell that the frame it adopted predates the metrics it just named.

`__FlushElementTree` therefore commits, and then either publishes — if a
painter has bound — or holds that frame and parks on the watch, exactly as
`adoptStyleSheet` parks on its response: the engine thread's tasks go on
running, no other job does, and the view's own cancellation token is the biased
first arm, so a release ends the wait with a throw rather than a frame. Waking
bound, it adopts the painter's metrics; if they moved the viewport the held
frame is discarded and the document is committed again, which is the resize
path, and the frame that goes out is at the painter's size either way. Only the
*first* binding is waited for — a painter that detaches leaves the last metrics
behind, so a view moved to the background never parks its group again.

Because boot's last act is a flush, **a view no painter ever binds publishes no
frame and reports no `ScriptFinished`**, and so never becomes ready. One more
task of the view consumes the watch, `consume_metrics`, and settles the page
once per change: that is what commits a resize with no JavaScript behind it.

`dom::Document<T>` privately owns its style/layout state, retained commit
builder and Vello scene; that DOM-side painter is distinct from the
embedder-thread `bobcat_core::Painter` that composes a published frame.
In Bobcat the payload is `()` and the core adds
the permanent `page` root plus Lynx UA defaults from `PageConfig`.

It also defines the three components the engine owns — `raw-text`, `image` and
the blur view — each in its own module (`tree::raw_text`, `tree::image` and
`tree::blur_view`, which own the component, its UA rules, and its tests
together). Lynx writes a
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

The blur view is the third join, and the shallowest: `blur-radius` becomes a
`backdrop-filter: blur(…)` presentational hint, so a property the cascade, the
stacking pass and the painter's backdrop bake already implement does all the
work. One component is installed under both tag names — native registers
`blur-view`, web-core registers `x-blur-view` and a compiled `.web.bundle`
writes that one verbatim — and the radius enters the cascade as a CSS length
rather than through web-core's `parseFloat`, so `rpx`, `em` and `vw` resolve
where web-core would have dropped them (a user ruling, recorded in
`docs/tracking/deviations.md`). The tag has no UA rules of its own: both names
are in the shared container list and in the `defaultOverflowVisible` rule,
because native's `LynxUIBlurView` extends `LynxUIView`.

```text
private Document<()>
  ├── DOM + Stylo arenas
  ├── layout/text state
  ├── ImageRegistry          (source names and load states; no pixels)
  └── private dom paint::Painter (main-thread commit builder)
        ├── retained Arc<CommittedFrame>   (paint tables + scroll-slot table +
        │                                   the split scene: per-space fragments
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
Lynx main thread, whose boot module created it out of the configuration it read
and the inputs that thread kept. That thread is the only committer.

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

- `ToMain`, an mpsc FIFO in: `DispatchEvent`, `BeginFrame { now, seq }`,
  `Refill { offsets }`, `ImageEvents`. A FIFO because the order two commands
  arrive in is what they mean. `LynxView` holds the one strong sender, inside
  the seat an attached `Painter` holds only a `Weak` of, so a painter can never
  keep a released view's task alive.
- `watch<Option<Viewport>>` on the same seat, written by the attached painter:
  the device metrics, which are observed state rather than history and are
  deliberately not a command — an unbound flush parks the job it runs in on
  this very watch, and no other job would run to read one.
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
holds up a sibling — but every entry into a realm is one job on that thread's
one queue, and the jobs run one at a time. A view parked on a synchronous
stylesheet therefore holds up every *realm* on the thread while holding up no
task of any of them. A second view costs no second heap,
no second module graph and no second set of Stylo workers, at the price of
the two never restyling in parallel. The assumption that buys is that a
person drives one view at a time. A host that needs two pages genuinely
parallel gives them a group each, on a thread each. The pool in particular
*must* be the group's rather than any view's: rayon takes the calling thread
over as index zero of the first pool built on it and refuses a second one
there forever, so one thread can only ever build one pool.

Dropping a view cancels its source work, detaches its image inbox, and closes
its command channel, which ends ordinary view tasks. The owner awaits MTS JS
disposal over the existing Worker inbox, then releases the realm and document;
remaining Worker senders close with it. The thread keeps serving its siblings. The group's two threads
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
    scroll/dispatch/BeginFrame
    compose: upload scene, acquire, present
    capture, offscreen ticks
  ── ToMain mpsc ──▶                  ◀── ViewNotice mpsc ──
  ── watch<Option<Viewport>> ──▶      ◀── watch<Published> ──
      (the painter's metrics;               (frame, listener names,
       the first write binds)                newest serviced BeginFrame)
                                      ◀── EventRequester wakeup ──
      Lynx main thread — the group's, shared by every view in it
                    (one task per wait; one runtime, one style pool)
                    the realm owns its document
                    PAPI mutations: plain &mut
                    __FlushElementTree: commit, then publish — or, before
                      the first binding, hold the frame and park
                      style → layout → build → encode
```

The surface is built on that thread and stays there:
`create_surface` from a window handle panics off the macOS main thread, and
the same thread is the one that will acquire, render and present into it. That
is why the target is an argument to `Painter::new` rather than something
attached later, and why a `Painter` is `!Send`. A view whose painter has
*detached* goes on running — it commits and publishes, and nothing draws — and
a painter may outlive the view it was watching, going on showing and capturing
the last frame it drew. A view no painter has bound **yet** is the other case:
it stops at its first flush until one does. A frame's
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

Every entry into a realm — input dispatches, scrolls, resource updates,
`BeginFrame` ticks, a module completion, a timer coming due, a metrics change,
a sibling's checkpoint — ends with a commit when anything went stale, which is
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
compose program tagged with the compose space each shape rides — its path of
scroll, sticky and animation nodes in the frame's one space tree — the content
between them lands in per-space scene fragments, and replaying the program
with a set of per-slot offsets reproduces exactly what a monolithic encode at
those offsets would have produced. A user scroll therefore never waits for a
commit — or the main thread at all. The painting side arbitrates
consumption against the published slot table, keeps the consumed offsets as
*scroll intents*, and recomposes and re-hit-tests at those offsets
immediately; between refills the intents *are* the offsets, and no per-event
command exists. When a frame publishes, an intent the frame's own offset
already equals has served its purpose and drops; the rest re-clamp to the
new bounds. The arbitration is `dom`'s own chain walk (`drive_chain`) over
each slot's published policy — `overscroll-behavior` fences the reach, the
engine's `scroll-capture: nearest` visits the container above first — and
each step lands per css-scroll-snap-1 from the slot's published snap
positions: a wheel tick steps to the next position, a drag is raw until its
release, when a `ScrollEnd` decision settles every container the drag moved
from where it found it. A frame's adoption also re-snaps every snapping
container no drag is holding, which is how the initial layout and a
relayout come to rest on a position.

Inertia is the same walk on the painter's clock (`paint/inertia.rs`): the
intents measure a drag's release velocity from the drag's own steps, and
each display frame the fling's distance since the last is chained from the
slot the drag latched as a fling step, decaying on `paint/motion.rs`'s curve
until what is left is under a physical pixel — aimed, on a snapping axis, at
the position its whole travel would settle on. A slot published with
`overscroll-behavior: contain-bounce` lets the intents stand past its edge:
a drag stretches it on the rubber band, a fling overshoots at the overshoot
decay, and once nothing holds it a bounce back per frame brings it home on
the critically damped spring. A drag's first step stops whatever is moving
on its chain and takes over. None of this leaves the painter: the router
decides what it always did, no event is involved, nothing recommits — a
stretch composes the edge's own content, asks for no refill, and the
writeback the next refill carries is clamped by the document.

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

### One render path: a scroll frame recomposes the committed fragments

There is one path from a committed frame to pixels. Every presented frame is
one flat vello scene: `compose_into` replays the commit's compose program —
pre-encoded per-space fragments plus the push/pop ops over them — with each
scroll slot translated by the offset the painter holds for it. Nothing is
retained per scroller, and a scroll frame's cost is bounded by the commit's
encode windows, which already discarded everything no clip chain admits.
Retained per-scroller textures were tried and removed: measured per scroll
frame they were slower than flat recomposition — each frame copied a plane
larger than the viewport into vello's image atlas — and every commit re-baked
all of them.

A scroll container is no stacking context by itself, as on the web and in
web-core (whose `scroll-view` is `position: relative` with no `z-index`;
`list` is one through its own `contain: layout`). Native Lynx scroll views are
compositing boundaries that cap a descendant's `z-index`; this engine does
not follow that. A scroller's content therefore need not be contiguous in the
compose program: a positioned or `z-index` descendant sorts in the enclosing
stacking context, and every record it produces still names the scroller's
scroll space and clip, which is all composition, culling and hit testing
read.

No frame materializes a whole composition beside its fragments (that would be
a content-proportional second encoding): `scene()` borrows the single fragment
of the common whole-frame shape and answers `None` for every other, and
consumers needing a flat scene compose one on demand.

### The one optional pre-step: `filter: blur()` and `backdrop-filter` bakes

That render path has exactly one conditional step in front of it, and one test
selects it: `CommittedFrame::filter_groups()` is empty for every frame of every
page that neither blurs nor filters a backdrop, and `compose_and_render` then
behaves exactly as above. When it is not empty, each entry is baked offscreen
and filtered before the frame composes, and the composition draws one texture
per entry.

The commit side stays device-free. A `FilterGroup` is σ and a rect in *device*
pixels plus a range of compose-program ops, and the two properties differ only
in what those mean.

For a **`filter: blur()` group** the rect is already 3σ larger than the group's
content on every side, so the bake's clamp-to-edge sampler reads the
transparent black filter-effects-1 specifies; the range is the ops the group
encloses, bracketed by `PushFilter`/`PopFilter`.

For a **`backdrop-filter` entry** the rect is exactly the element's transformed
border box — filter-effects-2 crops the Backdrop Root Image to it *before*
filtering, and the property enlarges no ink overflow, so there is no margin —
and the sampler is `MirrorRepeat`, which reflects the backdrop back in at that
crop rather than smearing an edge row or darkening toward a transparent
border. The range points **backwards**: from the content start of the
element's nearest Backdrop Root ancestor (`filter`, `opacity < 1`, `mask`,
`clip-path`, `mix-blend-mode`, `backdrop-filter`, a current `opacity` animation
at any reading, exported or not, or the root) to the element's own scope open —
exactly what was painted before it inside that root. A `will-change` naming one
of those properties is a Backdrop Root per the spec and is deliberately not one
here, since no group layer is opened per `will-change` element (recorded in
`docs/tracking/deviations.md`); the current `opacity` animation, which Web
Animations treats as `will-change: opacity`, already paints a group. Its one
op, `PushBackdrop`, has no matching pop; it sits innermost in the element's own
layers and before any of its items, so the element's `opacity`, `clip-path`,
`mask-image` and `filter` apply to the backdrop and to the element together,
and an element carrying both properties has its filtered backdrop baked
*inside* its own blur group.

Neither op encodes anything without a texture, which is what lets one program
serve both jobs: with a texture the bracket is one `draw_image` and the range
is skipped, or the backdrop is one fill through the element's own rounded
border box; without one the group's range replays raw and the frame is simply
**unblurred**, and the backdrop op draws nothing so the **unfiltered**
backdrop it sits on is what shows. `Document::scene()` and any consumer with
no GPU take that fallback by construction.

CSS clips a filter's *output* by the ancestors' `overflow` clips, not its
input, and the walker does the same. A group scope opens outside the clips its
enclosing scope's items pushed and its items push their chains again inside
it; for a scope whose element has `filter` or `backdrop-filter` — both
contain their positioned descendants, so every chain inside extends the
element's own — the chain is cut at that element's chain. The links between
the enclosing such scope's chain and this one are the scope's output clips,
ordinary `Push` ops outside its own layers, the innermost a full `SrcOver`
layer so the effect layer never opens directly inside a clip layer, and its
items push only the links below. So an ancestor's clip cuts the texture, the
raw fallback and the backdrop once, after the filter; content just past the
edge still blurs ink back inside it; and a nested backdrop reads its root's
content uncut by the root's ancestors. Every other group re-pushes whole
chains, which lets a fixed descendant of an `opacity` group escape the
group's ancestors' clips.

The device side is `dom::render::blur::FilterTextures`, one per
`vello::Renderer`, owned beside that renderer's `AtlasResidency` by `Headless`
and by the painter's `WindowGraphics`. Its cache holds one commit's bakes, and
each entry re-bakes on its own two conditional readings. The painter's scroll
generation counts when some op in the entry's range and the entry's own space
differ in their innermost scroll or sticky node, a node on one path and not
the other. A blurred scroller's content slides under the blur, and a backdrop
inside a scroller slides over what was painted outside it. A blurred box inside
a scroller re-bakes nothing: its range is its own content, and the scroller's
clip is an output clip outside it, so a scroll moves the texture under a still
clip. The timeline reading counts when a curve moves or fades some op in the
range relative to the entry. For a backdrop that is another element's curve in
its prefix, or its own element's transform curve. For a blur group it is a
curve on its own content; the element's own curves change no baked pixel — a
transform moves the texture across the ancestors' clips outside the range,
and an opacity-only curve applies where the texture is drawn. An entry whose
range draws a backdrop's texture also takes on that backdrop's two conditions:
an element with both properties draws its backdrop inside its own blur group,
and a child's backdrop range opens with its root's. So a tick re-bakes only
the entries that read it. Commit ids
restart per document, so a target pointed at a second document must `forget`
the cache, the same obligation it already has for its own compose key. Bakes
happen in increasing order of range end, so every texture an entry's own range
draws already exists when it bakes; each bake is a `render_to_texture` and therefore
owes the residency a pass of its own, because a bake of a solid-colour group
is precisely the patch-free render that frees vello's image atlas. The baked
textures are override images, so the *composite* render names them to the
residency too.

Filter memory is page-complexity-linear, so it is capped: 8192 device px per
texture side and a quarter of the atlas in total area, shared by both kinds of
entry and consumed in program order. An entry past the cap gets no texture and
takes the unfiltered fallback — the fallback is the budget's enforcement
mechanism, not an error path.

Neither property is exported as a composite curve, so an animated one
recommits and re-bakes every tick.

## Composite animations compose; the rest tick

The same compose machinery carries animations. At commit, an element whose
one running animation moves only `opacity`/`transform` — and whose keyframes
the exporter can re-express exactly (see `docs/tracking/css-animation.md`) —
publishes an `AnimationSlot` curve on the frame: timing from stylo's public
`Animation` fields, per-property tracks re-read from the stylist's
`@keyframes` steps. The curve is one animation node in the frame's compose
space tree, between the scroll and sticky nodes around and inside the
element, so clips, scroll containers and sticky boxes in the animated subtree
compose at their place on the path and refuse nothing. Each presented frame
samples the curve at the frame clock: the element's group alpha is replaced,
and the transform delta against the committed bake is the node's map, applied
to everything whose path passes through it — fragments, layer pushes, clips,
image draws, filter bakes, and hit tests. Between commits the compositor
animates alone. A curve with a transform track stays on the main thread,
its opacity track included, when its element's extent exceeds three viewport
areas, and when an enclosing composited group cannot bound it — only a scale
through 0 on the group's side.

The encode stays screen-bounded while content moves. Each transform track
carries its reach — per op, the range its parameters take over the curve's
whole domain — and culling pulls the viewport back through it, so a list of
rows each running its own animation encodes the rows that can reach the list's
window, and the clips and encode windows inside a moving subtree bound as
usual. Composition samples only the curves and sticky boxes the program
references. `has_live_curves` says the program references a curve, which is
what makes the painter recompose every frame; `has_exported_curves` says any
curve exported, which is what makes an input's hit test sample at the input's
instant.

The side effects Web Animations gives an in-effect `opacity`/`transform`
animation are keyed on the animation, not on its export: a stacking context
for either, a composited group and a Backdrop Root for `opacity`, and the
containing block of absolute and fixed descendants for `transform`. The
driver keeps them as two node bits and relayouts positioned descendants only
when the transform bit flips, so paint order and containment are the same on
both sides of a hand-over between the compositor and the main thread.

A window painter's `BeginFrame` narrows accordingly: it is sent per frame
only while the committed frame reports `needs_main_ticks` — something
animating that could not export — and, once a finite curve runs past its end,
per frame until the commit of its finish restyle is adopted. There an
infinite exported animation involves the main thread zero times per frame.
An offscreen `tick` does not narrow: it sends `BeginFrame` and waits on it
every call, so a headless host ticks the main thread each frame. An animation
**frozen** by css-contain-2 §4 narrows it all the way to nothing: an element in
a skipped subtree (`content-visibility: hidden`, or a non-relevant
`content-visibility: auto` box) does not advance its timeline, so the commit
reports neither `animations_active` nor `needs_main_ticks` for it and
`owes_frame`/`is_animating` stay false — a page whose only animations are
frozen is idle, and the host stops reading its display clock for it entirely.
The reveal — a style change, or the relevance flip the commit itself makes —
reactivates it in that same commit, and the first `BeginFrame` after it is
where the animation resumes, from exactly the progress the freeze found (the
driver carries its start times by every interval it slept through; see
`crates/dom/src/style/animation.rs`). The
sampling mirrors stylo's own progress computation exactly, so while a curve
is inside its domain the values composition shows between commits are the
values any commit's restyle lands on at the same instant. Past a finite
curve's end they need not be: the compositor holds the curve's end value,
whatever the fill mode, until the commit of the finish restyle is adopted —
at least one frame, since that commit follows an asynchronous `BeginFrame`.
With a `forwards` fill the restyle lands on that same value; without one it
returns to the base value, and those frames show the end value instead. A
second gap is value-level, open until the export takes more than one
animation: a finished animation that fills does not export, so where a
filling animation later in the `animation` list covers a running one, the
compositor samples the running curve while the main thread shows the fill
(`docs/tracking/css-animation.md`).

## Native and Wasm spawning

`LynxGroup::new` starts the group's two engine threads, and `create_lynx_view`
hands each view to a task on the first of them. The core selects the thread
builder at compile time:

```text
not wasm32  -> std::thread::Builder
wasm32      -> wasm_thread::Builder
```

Both engine threads are a `jobs.rs` `JsThread` — a tokio `current_thread`
runtime with a `LocalSet`, plus the job queue its top loop drains between two
turns of that scheduler — and both need to wait out their realms' timer
deadlines. Natively they enable tokio's time driver. On wasm32 that driver reads `std::time::Instant`, which
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
   `bobcat-main`'s report that the group's Stylo pool is built; its QuickJS
   runtime is built first, and one that failed is each view's
   `StartupFailed` rather than the group's error. `create_lynx_view` validates the fonts and default family into a
   `dom::TextContext`, creates the view's link, builds the per-view
   `ResourceFetcher` on the calling thread, hands that fetcher each author
   stylesheet in the order the view listed them and then the entry, sends the far half of the
   link — the text context and the answers among it — to that thread, and
   returns a loading view synchronously. The embedder then builds a `Painter`
   over the `DrawTarget` it named and attaches it, which imposes the painter's
   metrics on the view and is what releases its first frame.
2. The view's task queues one job before it spawns anything and waits for
   nothing: that job opens the realm, installs the host members, and evaluates
   `bobcat:boot` — all of it while the embedder is still building its painter,
   because none of it needs a host turn. Ordinary `LynxView::pump` turns hand
   the fetcher every *later* request instead, and its completions answer the
   tasks awaiting them.
3. Boot constructs the realm's `Document` over the page configuration written
   into it — which builds the private document from the create-time viewport,
   the text context and the style pool — and then imports the entry by its
   URL. Nothing parks for a startup source before the first flush: a task of
   the view completes the entry's module when the entry's answer arrives, and
   boot's first `__FlushElementTree` waits for each listed author sheet and
   mounts it, in listed order, before the document is styled, so several
   sheets cascade in listed order and no frame is published without them.
   `ScriptFinished` or `StartupFailed` reports the outcome through the
   lifecycle event path. Dropping the view cancels its pending resource work
   and ends any wait boot is inside; other views continue.
4. `__FlushElementTree` commits — style flush, layout, paint-order build,
   scene encode — publishes the `Arc<CommittedFrame>` on the view's
   `watch<Published>`, and wakes the embedder through its `EventRequester`.
5. The host answers with two turns. `Painter::pump` adopts the newest
   `Published` together with the pixels it draws and produces the frame: the
   `FrameClock` is sampled once, gesture deadlines resolve against it, the
   adopted scene is uploaded if it is new, and the frame
   presents. `LynxView::pump` then services the host's resource system and
   hands back the lifecycle events. While the latest frame reports an
   animation it cannot compose alone, each painter turn
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

## Named stylesheets

Boot initializes the JS runtime's `__Card__` with the entry response URL before
importing the entry. JS resolves the card alias into a CSS resource URL.
`__LoadStyleSheet` sends an optional `ResourceFetcher::preload_source` hint and
returns a JS handle associated only with the URL. Each `__AdoptStyleSheet` sends
an ordinary stylesheet request and synchronously mounts its response before
returning. The fetcher owns pending loads, caching and failures; core holds only
that call's response receiver. An unfinished request parks the job the adoption
is running in until completion or view cancellation: no JavaScript job runs
meanwhile, this realm's or a sibling's, while `bobcat-main`'s tasks — the ones
that route that very response among them — carry on. The embedder supplies text
or preparsed styles without exposing that choice to JS. Cache
ownership and synchronous failures are described in
[named stylesheet loading and adoption](named-styles-runtime.md).

## Data lifecycle

Initial preprocessing, update/reset, global-property snapshots and reload use
the existing MTS command and Worker links. Boot reads the processor switch from
PageConfig and the metrics from Viewport. The initial processor name crosses the
startup-data binding as a string, without serialization or source interpolation.
JS owns BTS initialization and sends its snapshot through postMessage.
Host updates are accepted once MTS boot finished and otherwise return
`NotReady`; the BTS still loading never refuses one. See
[data and global-property lifecycle](data-lifecycle-runtime.md) for the call
order, input ownership, live ESM bindings and framework boundary.
