# Runtime architecture

Bobcat exposes one runtime object to an embedder: `bobcat_core::LynxView`.
The document, Element-PAPI tree, script realm, renderer scheduler, and the
tree hand-off protocol are implementation state. An embedder supplies only
capabilities and OS facts:

- a `ResourceFetcher`;
- a transferable `ScriptEngineFactory`;
- an `EventRequester` for lifecycle wakeups;
- a draw target plus `FrameRequester`;
- optionally an `AnimationClock` type, when the host has a better reading of a
  frame's time than the platform clock, or wants a reproducible one;
- owned font bytes or already-decoded image pixels when those resources are
  registered explicitly;
- viewport/device metrics and normalized input events;
- platform initialization, worker bootstrap, clocks, and file/network IO.

The dependency graph is:

```text
bobcat-cli  ─┐
             ├──▶ bobcat-core ──▶ dom ─┬─▶ hughie
bobcat-wasm ─┘          │              ├─▶ vendor/stylo
                        │              └─▶ vello/wgpu
                        └──▶ quickjs-rust-bridge
                             [feature = "quickjs"; native adapter]

main-thread JavaScript
  ├──▶ main-thread-globals.js (shape-only runtime compatibility sinks)
  └──▶ element-papi.js (packages/bobcat-element, embedded by bobcat-core)
        └──▶ private bobcat host callbacks
              └──▶ private dom::Document<()> tree

bobcat-cli ──▶ lynx-template-decoder + winit
bobcat-wasm ──▶ wasm-bindgen + wasm_thread + embedded QuickJS
```

## Animation timeline

A view is generic over its timeline and names one at construction, the same way
it is generic over its `Window`: `LynxView<'window, W, C>`, where `C` is an
`AnimationClock`. Every reading is a direct call, there is no trait object, and
no timeline can be swapped in after the view exists.

`LynxView::new` names `SystemClock` — the platform's monotonic clock,
`std::time::Instant` natively and `web_time::Instant` on Wasm, the same split
`quickjs-rust-bridge` already uses. A host that arranges nothing therefore gets
running animations, and `bobcat-cli` names no clock at all.

`LynxView::with_animation_clock` is the constructor for a host that names its
own, and two do. A browser has a better reading than it could take itself:
`requestAnimationFrame` hands over the frame's timestamp, the instant the frame
is *for*, where reading a clock partway through producing the frame drifts and
jitters — so the Render Worker builds its view on an `Arc<ManualClock>` and
writes that `DOMHighResTimeStamp` into it each frame. Tests and scripted
offscreen capture do the same with their own `ManualClock`, for a reproducible
sequence. Both work because a shared clock is itself a clock: `AnimationClock`
is implemented for `Arc<T>`, so the host keeps a handle to the very clock its
view reads without the view holding a trait object.

Whichever is installed, the engine samples it once per frame on the presenting
side and hands that one value to the document, so every animation in a frame is
sampled at the same instant. `dom` itself still reads no clock — `now` is a
parameter to `Document::advance_animations`, which is what keeps the timeline
substitutable at all.

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
3. retains `lepusCode.root` or the XML main-thread body in its own
   `ResourceFetcher` under a URL;
4. constructs `LynxView` with the `PageConfig` and injected capabilities;
5. mounts bundle `StyleInfo` through the pre-parsed stylesheet arm or XML
   `<style>` through the CSS-text arm, then calls
   `LynxView::execute_script(url)`.

The browser reference embedder performs the same XML section mapping inside
its Render Worker after fetching one XML URL. Neither embedder executes the
optional background section yet because `bobcat-core` does not yet provide a
background-thread realm; both report that limitation explicitly.

`execute_script` resolves and fetches UTF-8 JavaScript through the injected
resource contract, then starts the engine-owned Lynx main thread. Script
completion is reported by `LynxView::pump` as
`EngineEvent::ScriptFinished`; the engine enqueues that event before invoking
the construction-time `EventRequester`, so the host can pump immediately
without polling. `execute_script_with_cancellation` accepts a public resource
`CancellationToken`; dropping the returned future cancels that same token and
unblocks cooperative resolver/fetcher work. `load_style_sheet(url)` is the matching URL-shaped API for author CSS: it
resolves and fetches through the same `ResourceFetcher`, which answers with
either CSS text or a `PreparsedStyleSheet` the host decoded itself, and mounts
the result as author-origin rules. Load order is cascade order.

## Public and private boundaries

The public facade is `LynxView<'window, W>`, with
`OffscreenLynxView` as its windowless alias. It relays input, resize, redraw,
frame-pump, target attachment, offscreen ticks, capture, cancellable script
startup, owned-font registration, and decoded-image URL registration. It exposes no
tree getter, document getter, renderer getter, script-realm handle, or
decomposition method.

The following types are private to `bobcat-core`:

- `Engine`, `SharedTree`, and `TreeGuard`;
- `MainThreadRuntime` and its Element-PAPI host implementation;
- `LynxDocument`, `Viewport`, and `new_document`;
- the concrete QuickJS realm adapter;
- image caches and the fetch→decode→cache loader.

This prevents an embedder from bypassing commit ordering, mutating the tree
beside JavaScript, retaining a document during presentation, submitting a
scene independently of the view, or evaluating code directly in the view's
realm.

## Injected JavaScript VM

`ScriptEngineFactory` is `Debug + Send + Sync`. It crosses to the eventual
Lynx main thread and creates one owner-thread-bound `Box<dyn ScriptEngine>`
there. The VM itself is intentionally not `Send`.

`ScriptEngine` is a small host-integration protocol:

- install a named leaf callback under a namespace;
- execute source with a source URL/name;
- expose the VM's optional garbage-collection operation.

The callback boundary carries only `HostValue` primitives. Objects, symbols,
functions, raw VM values, and DOM handles cannot cross it. Bobcat installs
the private `bobcat.*` callbacks, evaluates a core-owned classic script with
shape-only `lynx` and `SystemInfo` stubs, props, context/module, error,
performance, and lifecycle sinks, evaluates the embedded Element PAPI, wraps
the fetched main-thread source, then runs
`processData → renderPage → __FlushElementTree`.

Those sinks retain and deliver nothing. They make compiled main-thread chunks
installable before Bobcat has the corresponding runtime subsystems; they do
not create a background `lynxCoreInject` realm or hide missing Element PAPI
members such as `__AddClass`.

Entry evaluation is synchronous. QuickJS drains its owned pending-job queue at
each checkpoint before returning, including jobs queued by the entry script
and synchronous boot sequence. A persistent JavaScript event loop remains a
later runtime feature.

The default `quickjs` feature contributes only
`quickjs_engine_factory() -> Arc<dyn ScriptEngineFactory>`. QuickJS realm,
configuration, values, and runtime entry points remain private. With default
features disabled, an embedder supplies another factory. The browser embedder
enables QuickJS explicitly and passes this factory directly to `LynxView`.
The built-in factory has no execution deadline; the underlying bridge retains
an opt-in timeout for its direct users and tests.

## Document and rendering ownership

`dom::Document<T>` privately owns its style/layout state, retained painter,
Vello scene, and image store. In Bobcat the payload is `()` and the core adds
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
  └── private Painter
        ├── retained vello::Scene
        ├── reusable walk scratch
        └── ImageStore
```

The presenting side alone runs input routing, retained-scene production, GPU
submission, presentation, and capture. The public `EventRequester`, `Window`,
and `FrameRequester` traits describe lifecycle wakeup, draw-target, and frame
scheduling capabilities; they do not expose the engine that consumes them.

Image codecs are represented by the host-implemented `image::Decoder`
contract. Container sniffing, framing checks, decoded pixels, and sanitized
metadata are public; the resource-driven loader and its caches are
engine-owned and not publicly constructible. The `<image>` element has not yet
wired that decoder into automatic loading. Current reference decoders exercise
the standalone decode contract, and an embedder may install completed pixels
under a CSS URL through `LynxView::register_image_url`; the private engine owns
the corresponding `ImageStore` update and retained-scene refresh.

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

`LynxView::execute_script` always delegates VM creation and execution to an
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
the Wasm `LynxView`, Vello/wgpu objects, resource provider, and a thin
lifecycle wrapper around core's QuickJS `ScriptEngineFactory`; the factory
creates its owner-thread-bound realm inside the nested Lynx main Worker. No
direct create/append/drop/flush DOM API is exposed to JavaScript.

## Frame walkthrough

1. `LynxView::new` builds the private document from `PageConfig` and device
   metrics.
2. `execute_script(url)` fetches source through `ResourceFetcher` and spawns
   the target-specific Lynx main task.
3. The task creates the injected VM, installs Bobcat callbacks and Element
   PAPI, evaluates the named source, and runs the boot sequence.
4. `__FlushElementTree` performs style/layout commit, returns the document,
   and asks `FrameRequester` for a frame.
5. The presenting side non-blockingly drains buffered input and resize, then
   samples the installed `AnimationClock` once and advances every running
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
cargo check -p bobcat-core --no-default-features
cargo check -p bobcat-core --all-features
cargo check -p bobcat-core --target wasm32-unknown-unknown --no-default-features
cargo check -p bobcat-cli
cargo check -p bobcat-wasm --target wasm32-unknown-unknown
cargo check --workspace --all-targets
```
