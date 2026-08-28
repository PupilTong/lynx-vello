# Runtime architecture

Bobcat exposes one runtime object to an embedder: `bobcat_core::LynxView`.
The document, Element-PAPI tree, script realm, renderer scheduler, and the
tree hand-off protocol are implementation state. An embedder supplies only
capabilities and OS facts:

- a `ResourceFetcher`;
- an `ImageStore`, which owns every decoded image the view draws;
- an `EventRequester` for lifecycle wakeups;
- a draw target plus `FrameRequester`;
- owned font bytes when fonts are registered explicitly;
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
frame resolves — `service_gesture_clock` for armed `longpress` deadlines and
`advance_animations` for the timeline — so a gesture and an animation in the
same frame cannot disagree about when the frame is. Input arrival is the one
other reading, taken in `dispatch_input` at the moment the event arrives, which
is what keeps a sequence buffered behind a busy document at its real duration.

**Where the reading is taken.** A window frame is `WindowGraphics::acquire`,
then `render_to_target`, then `present`. Acquiring first is deliberate: under
`AutoVsync` the swap chain hands over an image only once one is free, so
`acquire` is the call that blocks, and every image in flight is another display
refresh between that wait and scan-out. Sampling the clock after it puts the
whole frame on the near side of the pipeline — what is sampled is the frame
being produced for the next refresh, not one produced a pipeline-depth earlier.
The wait still happens outside the tree borrow, so it blocks nobody. An
offscreen `tick` has no swap chain and samples immediately.

`dom` itself reads no clock: `now` is a parameter to
`Document::advance_animations`. That is what lets the presenting side decide
the instant, not the document.

Advancing an animation never crosses to the Lynx main thread. The presenting
side already holds the document between frames, and the tick is a Stylo
animation-only traversal of just the animating elements — no selector
matching, no snapshots, and no layout unless an animated property actually
moved a box. Starting and cancelling animations still belong to the main
thread, and ride the style flush it already runs at `__FlushElementTree`.

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
   in its own `ResourceFetcher` under a URL;
4. constructs `LynxView` with the `PageConfig` and injected capabilities;
5. mounts bundle `StyleInfo` through the pre-parsed stylesheet arm or XML
   `<style>` through the CSS-text arm, then calls
   `LynxView::execute_script(url)`.

The browser reference embedder performs the same XML section mapping inside
its Render Worker after fetching one XML URL. Neither embedder executes the
optional background section yet because `bobcat-core` does not yet provide a
background-thread realm; both report that limitation explicitly.

`execute_script` resolves and fetches the UTF-8 entry MTS module through the
injected resource contract, then starts the engine-owned Lynx main thread. The
resolved URL becomes the entry's exact module specifier. Boot completion is
reported by `LynxView::pump` as `EngineEvent::ScriptFinished` on success or
`EngineEvent::ScriptRunError` on a fatal boot-time failure. A platform failure
on the script owner thread may also report `ScriptRunError` after boot. The
engine enqueues every event before invoking the construction-time
`EventRequester`, so the host can pump immediately without polling.
`load_style_sheet(url)` is the matching URL-shaped API for author CSS: it
resolves and fetches through the same `ResourceFetcher`, which answers with
either CSS text or a `PreparsedStyleSheet` the host decoded itself, and mounts
the result as author-origin rules. Load order is cascade order. Requests carry
a specifier plus its optional base URL, not a semantic resource kind or
transport hints. The embedder locates bytes by normalized resolved URL;
`fetch_style_sheet` selects the stylesheet payload contract. Other buffered
loads use `fetch_resource`, and a `ResourceRequest` carries no response-size
limit; each fetcher owns the memory bound for the response it materializes.

## Public and private boundaries

The public facade is `LynxView<'window, W>`, with
`OffscreenLynxView` as its windowless alias. It relays input, resize, redraw,
frame-pump, target attachment, offscreen ticks, capture, script
startup, owned-font registration, and image-store installation and loads. It
exposes no
tree getter, document getter, renderer getter, script-realm handle, or
decomposition method.

The following types are private to `bobcat-core`:

- `Engine`, `SharedTree`, and `TreeGuard`;
- `MainThreadRuntime` and its Element-PAPI host implementation;
- `LynxDocument`, `Viewport`, and `new_document`;
- the concrete QuickJS realm adapter.

This prevents an embedder from bypassing commit ordering, mutating the tree
beside JavaScript, retaining a document during presentation, submitting a
scene independently of the view, or evaluating code directly in the view's
realm.

## The core-owned JavaScript engine

The script engine is not an injected capability. `bobcat-core` owns one
`QuickJS` realm outright, behind the crate-private `quickjs::ScriptEngine`,
which is created on the engine-owned Lynx main thread and never leaves it —
it is deliberately not `Send`, and nothing outside the crate can name it.

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
        ├── retained vello::Scene
        └── reusable walk scratch
```

The presenting side alone runs input routing, retained-scene production, GPU
submission, presentation, and capture. The public `EventRequester`, `Window`,
and `FrameRequester` traits describe lifecycle wakeup, draw-target, and frame
scheduling capabilities; they do not expose the engine that consumes them.

Images are the host-implemented `ImageStore` contract and nothing else. No
container sniffing, codec, cache, byte budget or eviction policy exists in this
workspace: an embedder installs a store with `LynxView::set_image_store`, and
the engine asks it for one image at a time by source string. The paint walk
calls only the store's non-blocking `peek`, because it runs on the presenting
thread between a swap-chain acquire and a present and can neither block nor
suspend; a miss paints nothing that frame, the same not-yet-loaded state a
browser shows. `LynxView::load_image` awaits the store's `get` outside the
frame and then invalidates the retained scene, and `prefetch_image` starts the
same work without waiting. The `<image>` element has not yet wired the store
into automatic loading.

## Tree hand-off and visibility

The engine and Lynx main thread share exactly one document through a private
slot:

```text
Lynx main thread                         embedder/presenting thread
factory creates owner-thread VM          opaque LynxView
first PAPI mutation: take document        input, scrolling, scene production
later mutations: plain &mut               GPU submission and present
flush: layout, return document ─────────▶ request/present next frame
```

A batch touches the slot only when taking and returning the document. While
the slot is empty, the presenting side never blocks: it buffers input,
retains the last target, and retries on a later frame. A half-applied batch is
therefore unobservable. At the end of every evaluation the runtime returns an
open batch even if script omitted `__FlushElementTree`, matching web-core's
live-DOM visibility.

## Native and Wasm spawning

`LynxView::execute_script` always delegates VM creation and module boot to an
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
process-global singleton. The npm facade keeps one live view in each
`BobcatRenderer`; `BobcatCanvas.reset()` drops that view and constructs a
replacement while retaining the Render Worker, transferred canvas, Wasm
instance, and resource state. It does not join the detached script Worker: the
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

1. `LynxView::new` builds the private document from `PageConfig` and device
   metrics.
2. `execute_script(url)` fetches the entry MTS source through
   `ResourceFetcher` and spawns the target-specific Lynx main task.
3. The task creates the QuickJS realm, installs Bobcat callbacks, preloads
   `bobcat:runtime`, `bobcat:element`, and the resolved entry URL, then runs the
   TLA-based `bobcat:boot` module.
4. `__FlushElementTree` performs style/layout commit, returns the document,
   and asks `FrameRequester` for a frame.
5. The presenting side non-blockingly drains buffered input and resize, then
   samples the engine's `FrameClock` once and advances every running
   animation to that instant, then renders the retained document scene and
   submits it to its attached target. An animation that is still running makes
   the presenting side ask for the next frame itself, so the loop sustains
   without waking the Lynx main thread; `LynxView::is_animating` reports the
   same fact to an offscreen host that drives its own cadence.
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
