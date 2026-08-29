# Runtime architecture

Bobcat exposes one runtime object to an embedder: `bobcat_core::LynxView`.
The document, Element-PAPI tree, script realm, renderer scheduler, and the
commit/publish protocol are implementation state. An embedder supplies only
capabilities and OS facts:

- a `ResourceFetcher`, borrowed for construction only;
- a `ViewSources`: owned font bytes, an optional default font family, an
  optional `ImageStore` — which owns every decoded image the view draws —
  author stylesheet URLs in cascade order, and the one entry MTS module URL;
- an `EventRequester` for lifecycle wakeups;
- a draw target plus `FrameRequester`;
- viewport/device metrics and normalized input events;
- platform initialization, worker bootstrap, and file/network IO.

No clock is among them: the animation timeline is engine-owned.

The dependency graph is:

```text
bobcat-cli  ─┐
             ├──▶ bobcat-core ──▶ dom ─┬─▶ hughie
bobcat-wasm ─┘          │              ├─▶ vendor/stylo
                        │              └─▶ vello/wgpu
                        └──▶ quickjs-rust-bridge

QuickJS preloaded ESM graph
  bobcat:boot
    ├──▶ bobcat:element (flush binding)
    └──▶ await import(resolved entry MTS URL)
          ├──▶ bobcat:runtime (packages/bobcat-element/src/main-thread-runtime.mjs)
          │     └── named compatibility exports + engine EventTarget
          └──▶ bobcat:element (packages/bobcat-element/src/element-papi.mjs)
                └──▶ bobcat-internal:host (native named function exports)
                      └──▶ private dom::Document<()> tree

bobcat-cli ──▶ lynx-template-decoder + winit
bobcat-wasm ──▶ wasm-bindgen + wasm_thread + embedded QuickJS
```

## Animation timeline

The engine owns it. `crate::clock::FrameClock` is private to `bobcat-core`,
concrete, and constructed by `Engine::new`: there is no trait, no type
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

**The frame's one reading.** `notify_redraw` and `tick` each call
`FrameClock::now_seconds` exactly once and pass that `f64` to everything the
frame resolves — `service_gesture_clock` for armed `longpress` deadlines on
the presenting side, and the `BeginFrame` command that carries the same
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
The wait blocks nobody: the presenting side touches no document and takes no
lock. An offscreen `tick` has no swap chain and samples immediately.

`dom` itself reads no clock: `now` is a parameter to
`Document::advance_animations`. That is what lets the presenting side decide
the instant, not the document.

Advancing an animation runs where the document is — the Lynx main thread,
its only home. The presenting side sends one `BeginFrame { now }` command per
frame while the latest committed frame reports an active animation, and the
main thread's recv loop advances the timeline — a Stylo animation-only
traversal of just the animating elements, no JavaScript involved — and
commits what changed. The published frame's
`animations_active` flag is what keeps the loop sustained: the presenting
side re-requests frames and keeps sending `BeginFrame` until a commit reports
the timeline idle. Starting and cancelling animations belong to the style
flush the main thread already runs at `__FlushElementTree`.

`bobcat-core` deliberately does not re-export `dom`. The lower-layer crates
remain independently usable libraries, but an application embedding Bobcat
cannot reach them through a running view.

## Startup boundary

Source/container IO is embedder work. `bobcat-core` does not fetch, decode, or
interpret `.web.bundle` containers or Lynx XML envelopes and has no public
decoder/parser types for either format.

For the native product, `bobcat-cli`:

1. reads the local input and content-sniffs Lynx XML versus a web bundle;
2. decodes a bundle with `lynx-template-decoder`, or parses XML with
   `lynx-xml`, then produces the corresponding `PageConfig`;
3. retains `lepusCode.root` or the XML main-thread body as an entry MTS module
   in its own `ResourceFetcher` under a URL, alongside bundle `StyleInfo` on
   the pre-parsed stylesheet arm or an XML `<style>` body on the CSS-text arm;
4. names both URLs in a `ViewSources` and awaits one `LynxView::new` with the
   `PageConfig` and the remaining injected capabilities, where any failure is
   one `CliError::StartView`.

The browser reference embedder performs the same XML section mapping inside
its Render Worker after fetching one XML URL. Neither embedder executes the
optional background section yet because `bobcat-core` does not yet provide a
background-thread realm; both report that limitation explicitly.

`LynxView::new` fetches every `ViewSources` URL through the injected resource
contract — the author stylesheets first, each answered with CSS text or a
host-decoded `PreparsedStyleSheet`, then the UTF-8 entry MTS module — mounts them
as author-origin rules on a fresh document, and starts the engine-owned Lynx main
thread over it before returning. The resolved entry URL becomes its exact module
specifier; a source that will not load, or a thread that will not start, yields
`LynxViewError` and no view. A default family neither the containers nor the
platform has fails with `EngineError::UnknownFontFamily`. Boot completion is
reported by `LynxView::pump` as `EngineEvent::ScriptFinished` on success or
`EngineEvent::ScriptRunError` on a fatal boot-time failure. A platform failure on
the Lynx main thread may also report `ScriptRunError` after boot. The engine
enqueues every event before invoking the construction-time `EventRequester`, so
the host can pump immediately without polling. Requests carry a specifier plus
its optional base URL, not a semantic resource kind or transport hints. The
embedder locates bytes by normalized resolved URL; `fetch_style_sheet` selects
the stylesheet payload contract. Other buffered loads use `fetch_resource`, and
a `ResourceRequest` carries no response-size limit; each fetcher owns the memory
bound for the response it materializes.

## Public and private boundaries

The public facade is `LynxView<'window, W>`, with `OffscreenLynxView` as its
windowless alias. It relays input, resize, redraw, frame-pump, target
attachment, offscreen ticks, capture, and image loads. It exposes no tree
getter, document getter, renderer getter, script-realm handle, decomposition
method, or way to mount a stylesheet or start a second entry module.

The following types are private to `bobcat-core`:

- `Engine`, `FrameHub`, and `MainCommand`;
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
crate-private `quickjs::ScriptEngine`, which is created on the engine-owned
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

`dom::Document<T>` privately owns its style/layout state, retained painter and
Vello scene, and holds the embedder's image store behind an `Arc`. In Bobcat
the payload is `()` and the core adds
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
  ├── Arc<dyn ImageStore>   (the embedder's; the document holds no pixels)
  └── private Painter
        ├── retained Arc<CommittedFrame>   (paint tables + scroll-slot table +
        │                                   the split scene: per-chain fragments
        │                                   and the compose program over them;
        │                                   the publish unit)
        └── reusable walk/build scratch
```

A commit — style flush, layout, paint-order build, scene encode — runs where
the document is and ends by publishing one immutable `Arc<CommittedFrame>`.
The presenting side is a compositor over the published frame: it routes
input and recognizes gestures against the frame's tables, uploads the scene,
submits, presents, and captures. The public `EventRequester`, `Window`, and
`FrameRequester` traits describe lifecycle wakeup, draw-target, and frame
scheduling capabilities; they do not expose the engine that consumes them.

Images are the host-implemented `ImageStore` contract and nothing else. No
container sniffing, codec, cache, byte budget or eviction policy exists in this
workspace: an embedder supplies a store as its `ViewSources::image_store`, and
the engine asks it for one image at a time by source string. The paint walk
calls only the store's non-blocking `peek`, because encoding runs inside a
commit on the document's owner thread and must not stall it; a miss paints
nothing that frame, the same not-yet-loaded state a browser shows. `LynxView::load_image` awaits the store's `get` outside the
frame and then invalidates the retained scene, and `prefetch_image` starts the
same work without waiting. The `<image>` element has not yet wired the store
into automatic loading.

## Commit, publish, and visibility

The document has one owner for its whole life: the engine-owned Lynx main
thread, which `execute_script` starts and which creates the document itself
from the view's `PageConfig` and device metrics. The engine never holds one —
setup called before `execute_script` buffers in the command channel and
applies, in send order, before the entry script boots. That thread is the
only committer, and the two threads share only two one-way channels:

```text
Lynx main thread (owns document + realm)   embedder/presenting thread
PAPI mutations: plain &mut                 input routing + gesture recognition
__FlushElementTree: commit                 (against the published frame)
  style → layout → build → encode          scroll/dispatch/resize/BeginFrame
  ─── publish Arc<CommittedFrame> ───▶     compose: upload scene, present
  ◀── MainCommand channel ─────────────    capture, offscreen ticks
```

Every command round the main thread serves — input dispatches, scrolls,
resizes, resource updates, `BeginFrame` ticks — ends with a commit when
anything went stale, which is what makes the recorded contract true: script
must flush after mutating, and nothing guarantees the tree is *not* flushed
at other times. A half-applied JavaScript turn is still unobservable, because
the main thread only serves commands between evaluations. The presenting
side never blocks and never skips a frame: it always has the latest published
frame to compose and hit-test, however busy the main thread is.

## Scroll composes; a refill recommits

The frame is baked *unscrolled*: the walker's layer-stack pushes become a
compose program tagged with the scroll chain each shape rides, the content
between them lands in per-chain scene fragments, and replaying the program
with a set of per-slot offsets reproduces exactly what a monolithic encode at
those offsets would have produced. A user scroll therefore never waits for a
commit. The presenting side arbitrates consumption against the published
slot table, keeps the consumed offsets as *scroll intents* (each stamped
with its `ScrollBy` command's sequence number), recomposes and re-hit-tests
at those offsets immediately, and sends the same `ScrollBy` to the main
thread, whose document applies it without dirtying anything. A published
frame echoes the highest sequence applied, so intents a frame already
incorporates are dropped and the rest re-clamp to its bounds.

The encode is windowed: each slot's fragments cover one scrollport past its
committed offset per scrollable axis (`ENCODE_WINDOW_SCROLLPORTS`). When an
intent moves past half its remaining window headroom, the engine sends one
`Refill` command per committed frame; the main thread marks the paint stale
and its next commit re-bakes the windows centered on the current offsets —
no script involvement anywhere. Programmatic scrolls need no refill today
because every scroll reaching the document rides a `ScrollBy` the intents
already display; a future script-facing scroll API must either dirty the
paint or publish its offsets, since the compositor only knows what crossed
the channel.

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

On Wasm, `configure_wasm_workers(worker_script_url, style_thread_count)` is
the OS bootstrap seam. It configures the default `wasm_thread` worker script;
the core then uses that same target-specific spawn path for the Lynx main
Worker and for Stylo's Rayon Workers. Stylo pool creation belongs to the core,
not the browser facade.

Wasm follows the native ownership model: every `LynxView` spawns and owns one
Lynx-main Worker, and dropping that view closes its command channel so the
Worker drops its QuickJS realm and exits. Independent views are not a
process-global singleton. The npm facade keeps at most one live view per
`BobcatRenderer`: `create` builds none, and each `load` replaces the view
before it, retaining the Render Worker, transferred canvas, Wasm instance, and
wrapper state. It does not join the detached script Worker: the
closed command channel makes that Worker drop its thread-bound realm and exit
naturally, and independent views may overlap during that brief teardown.

Only the Stylo Rayon pool is process-wide. It adopts the persistent Render
Worker as index zero rather than a view's transient Lynx-main Worker, and adds
at least one managed style Worker. A traversal entered from a script owner is
therefore transferred onto a managed pool worker, while presentation enters
from its long-lived index-zero owner. The configured count describes that
style pool; each live view's Lynx-main Worker is separate.

The browser UI thread is a JavaScript coordinator only. It creates an
embedder/Render Worker and transfers an `OffscreenCanvas`. That Worker owns
the Wasm `LynxView`, Vello/wgpu objects, and resource provider; core creates
its owner-thread-bound realm inside the nested Lynx main Worker. No direct
create/append/drop/flush DOM API is exposed to JavaScript.

## Frame walkthrough

1. `LynxView::new` fetches the `ViewSources`' stylesheets and entry MTS source
   through `ResourceFetcher`.
2. It builds the private document from `PageConfig`, device metrics, and those
   sheets, then spawns the target-specific Lynx main task over it — the
   document's owner for the rest of its life.
3. The task creates the QuickJS realm, installs Bobcat callbacks, preloads
   `bobcat:runtime`, `bobcat:element`, and the resolved entry URL, then runs the
   TLA-based `bobcat:boot` module.
4. `__FlushElementTree` commits — style flush, layout, paint-order build,
   scene encode — publishes the `Arc<CommittedFrame>`, and asks
   `FrameRequester` for a frame.
5. The presenting side samples the engine's `FrameClock` once per frame,
   resolves gesture deadlines against it, uploads the latest published scene
   if it is new, and presents. While the latest frame reports an active
   animation it sends the main thread one `BeginFrame` carrying that reading
   and keeps requesting frames, so the animation loop sustains without any
   JavaScript running; `LynxView::is_animating` reports the same fact to an
   offscreen host that drives its own cadence.
6. The task enqueues sanitized script completion and calls `EventRequester`;
   the awakened host observes it through `pump`. No realm or tree object
   crosses the boundary.

## Validation matrix

```sh
cargo check -p bobcat-core
cargo check -p bobcat-core --target wasm32-unknown-unknown
cargo check -p bobcat-cli
cargo check -p bobcat-wasm --target wasm32-unknown-unknown
cargo check --workspace --all-targets
```
