# Browser Wasm embedder

`crates/bobcat-wasm` is the `wasm-bindgen` browser embedder and npm facade for
`wasm32-unknown-unknown`. It builds with shared memory and uses crates.io
Vello 0.9/wgpu 29. It runs the same core-owned QuickJS engine as native
Bobcat.

## Execution and ownership

The browser has three roles:

```text
browser UI thread
  BobcatCanvas Promise facade only
  └── creates Render Worker and transfers OffscreenCanvas

Render Worker
  initializes shared Wasm
  owns opaque LynxView + resource registry
  owns Vello/wgpu/OffscreenCanvas
  └── core wasm_thread spawn -> Lynx main/VM Worker
                               ├── owner-thread-bound QuickJS realm
                               ├── Element PAPI + private document batches
                               └── core wasm_thread spawn -> Stylo workers
```

The UI thread never instantiates Wasm and never owns an engine, document,
tree, scene, GPU object, or Rust session registry. Its public operations are
limited to view creation with `PageConfig`, URL-based script, stylesheet, and
Lynx XML requests, font registration, resize, error observation, disposal, and
automatic pointer forwarding from the attached HTML canvas.

The Render Worker constructs `LynxView`, attaches the transferred canvas, and
calls `configure_wasm_workers` once. That core API configures the worker
bootstrap used by both the engine-owned Lynx main task and the private Stylo
Rayon pool. The Wasm embedder does not take a document owner or initialize
Stylo itself. One Wasm instance owns one `BobcatRenderer` and one configured
pool, while that renderer may own a sequence of non-overlapping native views.
`BobcatCanvas.reset()` drops the current view, closing its sole command sender,
and immediately constructs its replacement. The detached per-view Lynx-main
Worker drops its thread-bound realm and exits naturally; the independent
replacement does not join it. The process-wide pool adopts the persistent
Render Worker as index zero and remains valid across resets. The configured
count covers that owner plus at least one managed Stylo Worker; each live view's
Lynx-main Worker is separate. The public facade still creates one fresh Render
Worker and Wasm instance per `BobcatCanvas`, not per reset.

The transient Lynx-main Worker invokes Stylo from outside its Rayon pool, so
Stylo transfers that traversal's root closure onto a managed worker. The Render
Worker enters from its stable index-zero slot. This keeps view lifetime
independent from the shared pool without requiring an idle script Worker to
service style work.

## Resource and script boundaries

Browser fetch policy stays in JavaScript. For `executeScript(url)`, the Render
Worker uses browser `fetch`, reads the response stream with a 16 MiB bound,
and registers the raw entry-MTS bytes in its `BrowserResources`
implementation under the final response URL, then calls the opaque Rust
view's `execute_script(url)`. The view resolves and reads the bytes through
`ResourceFetcher`, performs strict UTF-8 validation, and uses that final URL as
the ESM entry specifier; it never receives a bundle decoder.

`loadLynxXml(url)` similarly fetches the source envelope once and decodes it
with the browser's replacement-mode UTF-8 `TextDecoder`, matching web-core's
raw XML loader. Rust's `lynx-xml` parser validates and extracts the sections in
the Render Worker. The source uses `<lynx engine-version="...">` and
`<script thread="main">` / `<script thread="background">`; legacy
attribute spellings are rejected. A present stylesheet is registered as CSS
text and mounted before the main-thread section is started; the returned
Promise uses the same engine-event completion path as `executeScript`. The
exported `LYNX_XML_PAGE_CONFIG` supplies the source format's fixed
`false`/`false`/`true` display/overflow/selector defaults, while callers may
still pass an intentional host override to `BobcatCanvas.create`.

Like `executeScript`, `loadLynxXml` is a one-shot entry-script operation for
the current native view. Once either entry point has started a script, another
call rejects before fetching or mounting XML CSS, so a failed repeated load
cannot mutate the running page's cascade. A caller that wants a new page first
calls `reset()`, which preserves the outer Worker, OffscreenCanvas, initialized
Wasm module, page configuration, latest device metrics, resource provider, and
registered font containers while replacing the private view, Lynx-main Worker,
VM, and document. A successfully selected embedder default font family is
restored after those font containers are registered on the replacement view.
The provider clears the old page's transient script and stylesheet bytes
during reset so repeated Blob-URL submissions do not accumulate stale sources.

The optional XML background section is retained under a URL derived from the
final XML response URL but is not executed. Bobcat does not yet have a
background-thread realm, so the browser reports this limitation explicitly.

The UI facade resolves relative script, stylesheet, and Lynx XML URLs against
the embedding document's `document.baseURI` before crossing the Worker boundary.
The Render Worker accepts only absolute URLs: resolving there against
`self.location` would incorrectly use the npm package/Worker URL as the base.

The browser names no script engine at all. Core creates its realm inside the
Lynx main Worker, so the realm remains owner-thread-bound and uses Bobcat's
primitive-only host callbacks. Raw QuickJS values, realm handles, numeric DOM
ids, and host callbacks are not surfaced by the npm facade.

Startup is one synchronous host boundary over an asynchronous ESM evaluation.
QuickJS preloads `bobcat:runtime`, `bobcat:element`, and the resolved entry URL;
the `bobcat:boot` module uses top-level await to import the entry before it
calls a present `globalThis.renderPage` or dispatches `__RenderPage` on the
realm-local EventTarget returned by `lynx.getEngine()`. It then flushes the
element tree. QuickJS drains its owned pending-job queue until that
module-evaluation promise settles, so boot completion is exactly the
`ScriptFinished` engine event. No browser microtask checkpoint or timer
interception participates in completion; the fallback listener is retained
inside the preloaded runtime ESM rather than by the browser.

There is no browser create/append/drop/flush/direct-stylesheet API. Element
mutation is reachable only from the fetched entry MTS module through the named
exports of `bobcat:element`. `registerFonts(bytes)` is a narrow resource
capability: it registers every usable face in an OpenType container through
the opaque view and returns the number accepted, without exposing the document
or text engine. `loadStyleSheet(url)` fetches CSS in the Render Worker,
registers the bytes, and mounts them as author-origin rules through the same
resource boundary the entry MTS module uses; sheets cascade in load order. The
stylesheet contract has a second arm for a host that already parsed its CSS,
but a browser host never does, so this embedder always takes the text arm.

The browser facade still does not decode `.web.bundle` containers. A caller
may execute suitable JavaScript by URL or load a raw Lynx XML source card;
bundle retrieval, decode, `PageConfig` parsing, and `StyleInfo` lowering remain
external work, exactly as in the native CLI — where the CLI does perform them
and hands core the pre-parsed arm.

## Pointer input

`transferControlToOffscreen()` transfers drawing control, not the DOM canvas's
event target. `BobcatCanvas` therefore retains the `HTMLCanvasElement` and
listens for active `pointerdown`/`pointermove`/`pointerup`/`pointercancel`
sequences itself. It accepts the primary mouse button and every touch or pen
contact, captures each accepted pointer until release, and treats unexpected
capture loss as cancellation. Hover-only moves stay on the UI thread. The
facade temporarily sets `touch-action: none` because Bobcat's gesture router,
not the embedding page, arbitrates tap against content scrolling; disposal
restores the previous inline value and removes every listener.

Client coordinates are mapped through the canvas's current bounding rectangle
into the latest logical viewport size, in CSS pixels. The UI sends that small,
flat input record to the Render Worker without waiting for a response. Input
shares the same ordered Worker queue as reset and resize, so it cannot re-enter
the Wasm wrapper while an asynchronous view replacement owns it and a pointer
following resize is interpreted in the metrics installed before it.

No timestamp crosses the seam. `BobcatRenderer::dispatchPointer` constructs
core's `InputEvent` and calls the opaque `LynxView::dispatch_input`, which
stamps the event's arrival from the engine's own clock — the same clock its
frames read — so a press after a long idle period cannot derive its `longpress`
deadline from the last rendered frame, and nothing has to agree on a time
origin. Reset releases active captures before replacing the view; disposal
stops input before terminating the Worker. Wheel input is not connected yet.

## Synchronization and rendering

The private document moves through core's `SharedTree` slot. A PAPI batch
takes the document; the Render Worker only tries a non-blocking borrow. An
open batch therefore cannot expose partial mutation or stall the last retained
frame. A shared atomic `FrameSignal` carries redraw requests from the Lynx main
Worker to the Render Worker, whose animation loop calls `renderIfRequested`
with no argument: the animation timeline is core's own `web_time` clock, read
once per frame on the Render Worker after the canvas surface hands over an
image. `requestAnimationFrame`'s `DOMHighResTimeStamp` is taken on the page's
main thread, before this Worker is woken and on a different time origin than
its `performance.now()`, so it is not the better reading it appears to be.
A separate lost-wake-safe event signal wakes a Promise whenever core queues an
engine event. Script completion therefore does not poll and does not depend on
rAF, so a hidden page cannot strand an `executeScript` Promise merely by
suspending drawing. The UI facade, nested VM Worker startup, and built-in
QuickJS adapter impose no wall-clock deadline on loading or execution. Fetch,
VM initialization, and script errors are still reported normally; work that
never completes remains pending until the worker fails or the view is
disposed.

The release Wasm build uses `panic=abort`. Script-visible node IDs and mutation
preconditions are checked before entering the DOM, producing JavaScript errors
without a Rust panic. An unexpected internal panic remains fatal; a one-time
panic hook reports it before the Lynx-main Worker aborts.

JavaScript `postMessage` is only the browser host boundary: initial canvas
transfer, URL-based requests, resize, lifecycle, and result/error delivery. It
is not a serialized DOM mirror or reconciliation protocol.

The workspace pins `wasm_thread` to the upstream `spawn_from_worker` change.
The crates.io release forwards nested spawns to a parent protocol handler that
an explicitly-created Render Worker does not have; the pinned implementation
creates the nested module Worker directly. Core selects
`wasm_thread::Builder` with `cfg(target_arch = "wasm32")`; native builds select
`std::thread::Builder`.

## Build and isolation

Build and verify the browser package with:

```sh
pnpm --filter bobcat-wasm build
```

The build script probes Clang by compiling a Wasm object with the complete C
target-feature set, including `-mbulk-memory-opt`, and then verifies `llvm-ar`
can archive that object before starting Cargo. Apple clang has no WebAssembly
backend; on macOS install Homebrew LLVM (`brew install llvm`). The script finds
the standard Homebrew locations automatically. Set `BOBCAT_WASM_LLVM_BIN` to
another LLVM `bin` directory, or set `CC_wasm32_unknown_unknown` and
`AR_wasm32_unknown_unknown` to override the compiler and archiver explicitly.

The package invokes `wasm-pack` for the `web` target. Release builds use the
workspace-pinned Binaryen 132 `wasm-opt` with `-Oz`; the build rejects any
other version rather than falling back to wasm-pack's older downloaded copy.
Every Rust/LLVM feature in `.cargo/config.toml` has an explicit Binaryen
counterpart, including threads, bulk memory, extended const, multivalue,
nontrapping float-to-int, reference types, SIMD, relaxed SIMD, sign extension,
and tail calls. Generated glue and Wasm live under
`crates/bobcat-wasm/pkg/` and are not checked in. The verification script
checks that optimization removed the debugging name section while preserving
`target_features`, shared imported/exported memory, the Worker-only Wasm
import, the URL execution/XML and private pointer-dispatch methods, and the
absence of the removed direct DOM API.

The `wasm32` target disables Parley's `complex-scripts` feature, while native
targets retain it. This keeps grapheme segmentation, shaping, and ordinary
Unicode line breaking, but omits ICU's CJK and Southeast Asian segmentation
dictionaries from the browser binary. Chinese and Japanese ordinary line
breaking remains available; Thai, Khmer, Lao, and Myanmar text can instead
fall back to cluster-level emergency breaks in constrained boxes, and their
intrinsic minimum width can be larger than with dictionary segmentation.

The browser target enables `parking_lot_core/nightly`; with Wasm atomics this
selects atomic wait/notify instead of the generic Wasm parker that panics on
contention. The workspace's pinned nightly and `.cargo/config.toml` rebuild
the threaded standard library and enable the Chrome-135 target feature set.

Shared memory and Wasm threads require a cross-origin-isolated page:

```text
Cross-Origin-Opener-Policy: same-origin
Cross-Origin-Embedder-Policy: require-corp
```

`packages/github-pages/public/coi-service-worker.js` provides those headers
for the demo. With `require-corp`, remote scripts and future image/font/bundle
resources must satisfy CORS or a compatible Cross-Origin-Resource-Policy.

The Pages shell exposes its Canvas and Lynx XML workspace views through the
`tab` query parameter. The XML view loads `demo.lynx.xml` into a text editor and
submits edits through a same-origin Blob URL. It creates and transfers the DOM
canvas only for the first render. Every later submit calls `reset()` to drop
and rebuild only native `LynxView`, then loads the new source through the same
warm `BobcatCanvas`; the Render Worker, transferred OffscreenCanvas, Wasm
instance, Stylo pool, and cached font bytes remain in place. The Blob URL is
revoked only after `loadLynxXml` settles.

Synchronous GPU readback remains absent because browser WebGPU map completion
is Promise-driven; native capture blocks on device polling. Browser capture
requires a separate asynchronous facade API.
