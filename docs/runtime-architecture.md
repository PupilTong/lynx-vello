# Runtime architecture

Bobcat exposes one runtime object to an embedder: `bobcat_core::LynxView`.
The document, Element-PAPI tree, script realm, renderer scheduler, and the
tree hand-off protocol are implementation state. An embedder supplies only
capabilities and OS facts:

- a `ResourceFetcher`;
- a transferable `ScriptEngineFactory`;
- an `EventRequester` for lifecycle wakeups;
- a draw target plus `FrameRequester`;
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
  └──▶ element-papi.js (packages/bobcat-element, embedded by bobcat-core)
        └──▶ private bobcat host callbacks
              └──▶ private dom::Document<()> tree

bobcat-cli ──▶ lynx-template-decoder + winit
bobcat-wasm ──▶ wasm-bindgen + wasm_thread + embedded QuickJS
```

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
the private `bobcat.*` callbacks, evaluates the embedded Element PAPI, wraps
the fetched main-thread source, then runs
`processData → renderPage → __FlushElementTree`.

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

The Stylo pool includes the Lynx-main owner thread, so one shared Wasm instance
accepts one view. The npm facade satisfies this by creating a fresh Render
Worker and Wasm instance for every `BobcatCanvas`; a second view in the same
instance is rejected explicitly. The pool has at least two threads: the
entry-task owner at index zero plus one managed worker that remains available
after the synchronous entry task exits.

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
5. The presenting side non-blockingly renders the retained document scene and
   submits it to its attached target.
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
