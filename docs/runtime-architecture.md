# Runtime architecture

Bobcat exposes one runtime object to an embedder: `bobcat_core::LynxView`.
The document, Element-PAPI tree, script realm, renderer scheduler, and the
commit/publish protocol are implementation state. An embedder supplies only
capabilities and OS facts:

- a `ViewSources` — page config, owned font bytes, an optional default font
  family, author stylesheet URLs in cascade order, and the one entry MTS
  module URL — and, as a separate argument, the builder of the view's
  `ResourceFetcher`, which is also its `FrameImages` and owns every byte and
  pixel the view ever loads (`crates/bobcat-resources` is the reference
  implementation all shipped embedders use);
- an `EventRequester`, the one wakeup the engine has for the embedder's
  thread. It is a platform type, not a trait object — `LynxView::new` is
  generic over it and the Lynx main thread holds it, so the wake is a direct
  call; `NoWakeup` is the implementation for a host with no event loop to
  wake;
- a `DrawTarget`, at construction: `DrawTarget::window(...)` over anything
  convertible into a `WindowTarget` — a `'static` surface target, so a shared
  window handle rather than a borrow — or `DrawTarget::Offscreen` for a
  windowless GPU target. There is no attaching one later;
- viewport/device metrics and normalized input events;
- platform initialization, worker bootstrap, and file/network IO.

No clock is among them: the animation timeline is engine-owned.

The source tree mirrors the two runtime owners:

```text
crates/bobcat-core/src/
  view/lib.rs          LynxView, ViewSources, the startup guard, and the
                       shared values, messages, and link construction
  paint/lib.rs         Painter, frame clock, and painter-owned link replicas
  paint/sources.rs     painter-owned startup resource loading
  paint/gesture.rs     input arbitration
  paint/graphics.rs    window GPU state
  main/lib.rs          document creation, loaded-source mounting, startup,
                       and inbox
  main/quickjs.rs      owner-thread-bound QuickJS adapter
  main/runtime/lib.rs  realm/DOM integration
  main/tree/lib.rs     Lynx document and UA component policy
```

Shared command, event, viewport, and link vocabulary stays in `view` beside
the public handle that owns one end of it; a stateful type whose owner is
fixed lives under `paint` or `main`. Construction splits `ViewSources` once:
document inputs move into the main-owned startup request, while entry and
stylesheet specifiers stay with the embedder-owned painter and its
`ResourceFetcher`. Fetched source bytes cross to `main`; the fetcher, caches,
and decoded images never do.

The dependency graph is:

```text
bobcat-cli    ─┬──▶ bobcat-source (runtime + native-bundle)
               └──▶ bobcat-resources ─┐
bobcat-server ─┬──▶ bobcat-source     ├──▶ bobcat-core ──▶ dom ─┬─▶ hughie
               └──▶ bobcat-resources ─┤          │              ├─▶ vendor/stylo
bobcat-wasm ──┬───▶ bobcat-resources ─┘          │              └─▶ vello/wgpu
              └───▶ bobcat-source (runtime/XML only)
bobcat-source ────────────────────────────────────┘
                                                  └──▶ quickjs-rust-bridge

QuickJS preloaded ESM graph
  bobcat:boot
    ├──▶ bobcat:element (flush binding)
    ├──▶ bobcat:timers (timer-global installation)
    └──▶ await import(resolved entry MTS URL)
          ├──▶ bobcat:runtime (packages/bobcat-element/src/main-thread-runtime.mjs)
          │     └── named compatibility exports + engine EventTarget
          └──▶ bobcat:element (packages/bobcat-element/src/element-papi.mjs)
                └──▶ bobcat-internal:host (native named function exports)
                      └──▶ private dom::Document<()> tree

bobcat-cli ──▶ bobcat-source + winit
bobcat-wasm ──▶ bobcat-source (runtime/XML only) + wasm-bindgen + wasm_thread
bobcat-server ──▶ axum + reqwest + image/bmp
```

## Animation timeline

The engine owns it. `crate::paint::FrameClock` is private to `bobcat-core`,
concrete, and constructed with the `Painter`: there is no trait, no type
parameter, and no constructor that takes one. It reads the platform's monotonic
clock — `std::time::Instant` natively and `web_time::Instant` on Wasm, the same
split `quickjs-rust-bridge` already uses — with the epoch at view construction.
A host that arranges nothing gets running animations, because arranging nothing
is the only option.

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
its only home. The painting side sends one `BeginFrame { now }` command per
frame while the latest committed frame reports an active animation, and the
main thread's recv loop advances the timeline — a Stylo animation-only
traversal of just the animating elements, no JavaScript involved — and
commits what changed. The published frame's
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
2. `bobcat-source::PageSource` content-sniffs Lynx XML versus a web bundle,
   uses the shared `web` and `xml` parsers, lowers bundle
   `StyleInfo` directly to a `PreparsedStyleSheet`, and produces the
   corresponding `PageConfig`, source URLs, and payloads;
3. the embedder registers `lepusCode.root` or the XML main-thread body as the
   entry MTS module in its `bobcat-resources` instance, alongside the bundle
   stylesheet on the pre-parsed arm or an XML `<style>` body on the CSS-text
   arm; that resource system also owns every later file/network fetch, cache,
   and image decode;
4. the embedder awaits one `LynxView::new`, passing `DrawTarget`, the
   `bobcat-resources` per-view builder, and the resulting `ViewSources`.

The server enables `runtime` and `web-bundle` on the existing source crate.
Native-bundle decoding remains disabled for this screenshot product.

`bobcat-server` keeps HTTP handling concurrent but sends accepted captures
through a bounded FIFO to one dedicated capture thread. For each job that
thread is the embedder owner: it constructs the non-`Send` `LynxView` with an
800×600 DPR-1 `DrawTarget::Offscreen`, owns the view's `Painter` and all GPU
work, settles the page, captures tightly packed RGBA8, and destroys the view.
The Lynx main thread it spawns is the view's only other runtime thread; the
server adds no separate rendering owner. The HTTP side then encodes the frame
as a uncompressed BMP on Tokio's blocking pool after compositing over white, so
CPU encoding neither retains the view nor occupies the GPU lane. A worker
panic makes health fail and initiates server shutdown; queue saturation fails
admission rather than creating unbounded GPU work. This is a trusted-page
embedder: the public core deliberately exposes no QuickJS interrupt, and
`timeoutMs` cannot preempt synchronous script execution, a blocking GPU call
on the capture/embedder thread, or synchronous view teardown while it joins
the Lynx main thread.

The browser reference embedder uses the shared `register_lynx_xml_response` adapter inside
its Render Worker after fetching one XML URL. No shipped embedder executes the
optional background section yet because `bobcat-core` does not yet provide a
background-thread realm; each source-facing embedder reports that limitation
explicitly.

`LynxView::new` validates the viewport, creates the one link, starts
`bobcat-main`, builds the painter and the `DrawTarget` it was given on the
calling thread, and asynchronously awaits one startup result.
`bobcat-main` creates the fresh document itself, registers fonts, receives
each author stylesheet — fetched by the painter through the view's
`ResourceFetcher` — and mounts those sheets in cascade order, receives the
UTF-8 entry MTS module, creates QuickJS, and completes boot before answering.
The actual network or file IO may run wherever the fetcher chooses; fetch
futures are driven by the embedder-owned painter during construction, then
only the loaded source crosses the link. Every document mutation and
post-fetch runtime action remains on `bobcat-main`.

The resolved entry URL becomes its exact module specifier. A resource,
encoding, font, realm, script-boot, or thread failure yields `LynxViewError`
and no view. Dropping the unresolved constructor cancels pending resource
work or stops startup before `QuickJS` begins, releases the painter it built,
and directly joins `bobcat-main`. Synchronous startup JavaScript is not
externally interrupted, so teardown waits for it to return. Successful
construction has already completed boot; `EngineEvent::ScriptFinished`
remains queued for compatibility with the host's lifecycle loop.
`ScriptRunError` is reserved for
a fatal owner-thread failure after startup, while `ListenerFailed` remains
non-fatal. The engine enqueues every event before invoking the
construction-time `EventRequester`, so the host can pump immediately without
polling. Requests carry a specifier plus its optional base URL, not a semantic
resource kind or transport hints. The embedder locates bytes by normalized
resolved URL; `fetch_style_sheet` selects the stylesheet payload contract.
Other buffered loads use `fetch_resource`, and a `ResourceRequest` carries no
response-size limit; each fetcher owns the memory bound for the response it
materializes.

## Public and private boundaries

The public facade is `LynxView`. It applies input, resize, occlusion,
offscreen ticks, capture and image loads, and its `pump` is the
view's turn: it draws the frame the view owes and drains the lifecycle events
that turn produced. It exposes no tree getter, document getter, renderer
getter, script-realm handle, decomposition method, or way to mount a
stylesheet or start a second entry module. `LynxView<F>` carries only the
painter-owned `ResourceFetcher` type. The embedder's event-loop wakeup is a
separate constructor generic held by `bobcat-main`, not another owner or
thread represented in the view type.

The following types are private to `bobcat-core`:

- the `Painter` — everything the view draws with, owned by the view;
- the link — `ToMain`, `ToPainter`, and the frame mailbox
  behind them;
- `MainThreadRuntime` and its Element-PAPI host implementation;
- `LynxDocument`, `Viewport`, and `new_document`;
- the concrete QuickJS realm adapter.

This prevents an embedder from bypassing commit ordering, mutating the tree
beside JavaScript, reaching the main-thread document at all, submitting a
scene independently of the view, or evaluating code directly in the view's
realm.

## The core-owned JavaScript engine

The script engine is not an injected capability. `bobcat-core` owns one
`QuickJS` runtime and the single realm on it outright, behind the
crate-private `main::quickjs::ScriptEngine`, which is created on the engine-owned
Lynx main thread and never leaves it — it is deliberately not `Send`, and
nothing outside the crate can name it. The bridge would carry more realms on
that one runtime, which is the shape a background-thread realm would take:
its own global object and native modules, sharing the runtime's heap, job
queue, and execution limits, with no value crossing between the two.

Its whole surface is five operations:

- register a Rust-backed named function export in a native ESM module;
- register UTF-8 source under an exact preloaded module specifier;
- execute an ESM entry and wait for its evaluation promise to settle;
- call a named export of an already-loaded source module;
- run a collection.

The callback boundary carries only `quickjs-rust-bridge`'s primitive
`HostValue`/`HostArgument`. Objects, symbols,
functions, raw VM values, and DOM handles cannot cross it. Bobcat registers its
private callbacks as named exports of the native `bobcat-internal:host` ESM,
then preloads three kinds of ESM source: the core-owned `bobcat:runtime` named
compatibility exports, the embedded `bobcat:element` named Element-PAPI
exports, and the fetched entry under its resolved URL. `bobcat:element`
imports its native operations directly; nothing is installed as
`globalThis.bobcat`. Before registering the entry, core prepends its runtime
and Element-PAPI import declarations. Event delivery travels back through the
loaded `bobcat:element` namespace's `__BobcatDispatchEvent` export.

The final `bobcat:boot` module imports `lynx` and the flush binding from the
two built-ins; the transformed entry itself statically imports both built-ins.
Boot then runs:

```js
await import(entryMtsUrl);
const data = globalThis.processData?.(undefined);
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
fallback's `__RenderPage` delivery today. The other context sinks retain and
deliver nothing. They make compiled main-thread chunks installable before
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

`dom::Document<T>` privately owns its style/layout state, retained commit
builder and Vello scene; that DOM-side painter is distinct from the
embedder-thread `bobcat_core::paint::Painter` that composes a published frame.
In Bobcat the payload is `()` and the core adds
the permanent `page` root plus Lynx UA defaults from `PageConfig`.

It also defines the one component the engine owns, `raw-text`, in its own
module (`tree::raw_text`, which owns the component, its UA rules, and its
tests together). Lynx writes a
text run as an attribute (`__CreateRawText(value)` sets `text` on a `raw-text`
element) while everything downstream — Parley shaping, line breaking, the
glyph painter — speaks the W3C text node, so the component observes `text` and
reflects its value into one text node, updating that node in place and
carrying none at all for an empty value. The UA sheet supplies the display
policy the reflection needs: `text` is a flex container, `wrapper` is
`display: contents`, and a `raw-text` dissolves into the `text` it is written
inside and generates no box anywhere else.

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
the document is and ends by publishing one immutable `Arc<CommittedFrame>`.
The painter is a compositor over the published frame: it routes input and
recognizes gestures against the frame's tables, uploads the scene, submits,
presents, and captures. The public `EventRequester` trait describes the host
wakeup — implemented by the platform and named at `new`, never boxed — and it
does not expose the engine that consumes it. There is no frame-scheduling
capability: a commit records that it wants a frame and wakes the embedder,
whose next `pump` draws it — no OS frame callback is asked for or waited on.

Images are the host's `ResourceFetcher` and nothing else; no container
sniffing, codec, cache, byte budget or eviction policy exists in `bobcat-core`
or `dom`. The paint walk names each source it meets; the painter asks the
fetcher for it (`request_image`), the fetcher answers through the view's
`ImageReports` with the intrinsic size, and the document records the load and
recommits. A frame that names a source not yet loaded paints nothing for it,
the same not-yet-loaded state a browser shows. Each painter turn gives the
fetcher a moment of its own (`service_images`) to forward loads that completed
elsewhere. Composition then reads the frame's images once per commit through
`FrameImages::read`, synchronously and off the swap-chain window, each with the
`ImageSizeHint` of its largest draw in the frame — the size a host decodes to.
That read may block, because after a reported load it must not miss: a store
that evicted a bitmap restores it inside the call. `crates/bobcat-resources`
is the reference implementation of all of this, and `LynxView::prefetch_images`
warms sources ahead of the walk that would discover them. The `<image>`
element has not yet wired automatic loading.

## Commit, publish, and visibility

The document has one owner for its whole life: the engine-owned Lynx main
thread, which creates the document itself from the view's `PageConfig` and
device metrics. That thread is the only committer.

A view spans two threads and one link. The embedder's own thread — whichever
one constructed the view — owns the window, captures input, creates the
surface (the one call macOS allows nowhere else), and owns everything that
draws. The Lynx main thread owns the document and the realm. The link is an
ordered FIFO per direction plus a one-slot mailbox for the frames themselves,
and one typed wakeup back to the embedder.

```text
the thread that called LynxView::new (AppKit main, or a Render Worker)
  window lifecycle, input capture, surface creation
  LynxView, owning the Painter — everything below runs inside the
  embedder's own calls:
    input routing + gesture recognition (against the published frame)
    scroll/dispatch/resize/BeginFrame
    compose: upload scene, acquire, present
    capture, offscreen ticks
  ── ToMain FIFO ──▶                       ◀── ToPainter FIFO ──
                                           ◀── Arc<CommittedFrame> mailbox ──
                                           ◀── EventRequester wakeup ──
                    Lynx main thread (owns document + realm)
                    PAPI mutations: plain &mut
                    __FlushElementTree: commit
                      style → layout → build → encode
```

The surface is built on that thread, during construction, and stays there:
`create_surface` from a window handle panics off the macOS main thread, and
the same thread is the one that will acquire, render and present into it. A
view is therefore never in a state where it has run but has nowhere to put a
frame — the target is an argument to `new`, and there is no attaching one
afterwards. A frame's
vsync wait therefore lands inside the embedder's own turn, which is why the
embedder draws where a wait for the display is acceptable rather than inside
every input relay. Nothing about this differs by platform any more: the
browser, where `wgpu`'s handles are not `Send` under shared memory and an
`OffscreenCanvas` cannot be transferred on again, always had exactly this
shape, and the Render Worker is simply the thread that constructs the view.

A commit writes its frame into the mailbox, over whatever the painting side
has not read, and announces it as one `ToPainter::FrameChanged`; lifecycle
events, listener-name edges, and `BeginFrame` acknowledgements ride the same
FIFO in order. Frames stay out of it deliberately: a queue of them would
retain every intermediate scene, while a mailbox bounds the frames in flight
at one however far the main thread runs ahead. The painting side drains the
FIFO once per pass, reads the mailbox at most once per drain, and keeps the
name replica, the pending-redraw bit, and the newest frame locally — so
composing, hit-testing, and refilling take no lock at all. Every send from the
main thread wakes the embedder through its `EventRequester`, always after the
state it announces is in place, and the `pump` that answers is the turn that
draws it. A frame the painter asks of *itself* wakes nothing — it is asked
inside one of the embedder's own calls, and the turn that embedder is already
in is the turn that answers it. Nothing announces the main thread's exit:
dropping its end of the link closes the FIFO, which is the same fact. The view
announces its own, explicitly, because the main thread's command loop returns
on that message and a `Drop` that only released the FIFO would race it.

Every command round the main thread serves — input dispatches, scrolls,
resizes, resource updates, `BeginFrame` ticks — ends with a commit when
anything went stale, which is what makes the recorded contract true: script
must flush after mutating, and nothing guarantees the tree is *not* flushed
at other times. A half-applied JavaScript turn is still unobservable, because
the main thread only serves commands between evaluations. The painting
side never blocks and never skips a frame: it always has the latest published
frame to compose and hit-test, however busy the main thread is.

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

`LynxView::new` always delegates VM creation and module boot to an
engine-owned task. The core selects the thread builder at compile time:

```text
not wasm32  -> std::thread::Builder
wasm32      -> wasm_thread::Builder
```

On Wasm, `configure_wasm_workers(worker_script_url)` is the OS bootstrap
seam. It configures the default `wasm_thread` worker script and nothing else;
the core then uses that same target-specific spawn path for the Lynx main
Worker and for each view's Stylo Rayon Workers. Stylo pool creation belongs
to the core, not the browser facade — the facade only says how many workers a
view should get, through `ViewSources::style_threads`.

Wasm and native follow the same ownership model: every `LynxView` spawns and
owns one Lynx-main Worker — the Render Worker is the thread that constructed
the view, so it is where the painting happens — and dropping that view
explicitly shuts down and joins the Worker after it drops its document and
QuickJS realm. Independent
views are not a process-global singleton. The npm facade keeps at most one
live view per `BobcatRenderer`: `create` builds none, and each `load` replaces
the view before it, retaining the Render Worker, transferred canvas, Wasm
instance, and wrapper state. Replacement construction therefore cannot
overlap the old view's teardown.

Nothing about Stylo is process-wide any more. Each view's Lynx-main Worker
builds its own Rayon style pool as its first act of startup, and the pool
retires with the view. That Worker is index zero of the pool it builds, taken
over in place by rayon's `use_current_thread` — which is why the pool can only
be built there, and why the configured count includes it. Stylo's global pool
was built the same way and Gecko relies on it, so a lone view restyles on the
same threads and with the same parallelism it did before these pools became
per-view: the root closure runs inline on the script owner, and the managed
members take over only where a level is wider than the traversal's work unit.
The takeover is permanent — rayon leaks about 25 KB per pool and refuses a
second pool on the same thread forever — which is affordable only because the
Lynx-main Worker is created for one view and dies with it. Disjoint pools are
what let two views on two threads restyle at once; the process-wide traversal
mutex that serialized them is gone. `dom::MAX_STYLE_THREADS` (six) is a hard
ceiling, not a preference: Stylo indexes its per-traversal thread-local
storage by Rayon thread index into an array that long, and counts its own six
the same way.

The browser UI thread is a JavaScript coordinator only. It creates an
embedder/Render Worker and transfers an `OffscreenCanvas`. That Worker owns
the Wasm `LynxView`, Vello/wgpu objects, and resource provider; core creates
its owner-thread-bound realm inside the nested Lynx main Worker. No direct
create/append/drop/flush DOM API is exposed to JavaScript.

## Frame walkthrough

1. `LynxView::new` validates the metrics, creates the link, starts
   `bobcat-main`, and builds the painter — including the `DrawTarget` the
   embedder named and the per-view `ResourceFetcher` — on the calling thread.
2. `bobcat-main` creates the private document from `PageConfig` and the device
   metrics and registers fonts. In parallel, the calling thread drives the
   painter-owned fetcher for each stylesheet and then the entry MTS source,
   sending only loaded sources across the link in that order.
3. `bobcat-main` mounts each received sheet, creates the QuickJS realm on entry
   arrival, installs Bobcat callbacks, preloads `bobcat:runtime`,
   `bobcat:element`, `bobcat:timers`, and the resolved entry URL, then runs the
   TLA-based `bobcat:boot` module. Only complete success sends `Started` back
   through the same painter inbox. Cancelling construction drops pending
   resource futures on the calling thread, releases the painter, tells
   `bobcat-main` to stop, and joins it.
4. `__FlushElementTree` commits — style flush, layout, paint-order build,
   scene encode — writes the `Arc<CommittedFrame>` into the mailbox, announces
   it on the `ToPainter` FIFO, and wakes the embedder through its
   `EventRequester`.
5. The `pump` that answers that wakeup takes the pending frame and produces
   it: the `FrameClock` is sampled once, gesture deadlines resolve against it,
   the latest published scene is uploaded if it is new, and the frame
   presents. While the latest frame reports an active animation each turn
   sends the main thread one `BeginFrame` carrying that reading, and the loop
   sustains without any JavaScript and without waking anyone: `owes_frame`
   answers yes, and the embedder takes the next turn at its own display frame
   — a `CVDisplayLink` on the window's monitor natively, `requestAnimationFrame`
   in a Worker. The engine names no interval, and an offscreen host, which has
   no display to pace against, reads `LynxView::is_animating` instead.
6. The successful boot notification remains queued behind the same wakeup;
   the awakened host observes it through `pump`, which hands it back with
   whatever else the turn produced. A draw that fails arrives the same way,
   once, as `EngineEvent::RenderFailed`. No realm or tree object crosses the
   boundary.

## Validation matrix

```sh
cargo check -p bobcat-core
cargo check -p bobcat-core --target wasm32-unknown-unknown
cargo check -p bobcat-source
cargo check -p bobcat-cli
cargo check -p bobcat-server
cargo check -p bobcat-wasm --target wasm32-unknown-unknown
cargo check --workspace --all-targets
```
