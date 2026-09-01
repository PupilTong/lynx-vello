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
  owns opaque LynxView + resource registry — and, because it is the thread
  that constructed the view, it is the thread the view paints on
  owns Vello/wgpu/OffscreenCanvas
  └── core wasm_thread spawn -> Lynx main/VM Worker
                               ├── owner-thread-bound QuickJS realm
                               ├── Element PAPI + private document batches
                               └── core wasm_thread spawn -> Stylo workers
```

The UI thread never instantiates Wasm and never owns an engine, document,
tree, scene, GPU object, or Rust session registry. Its public operations are
limited to canvas creation with `PageConfig`, URL-based page loads, font and
default-family registration, resize, error observation, disposal, and
automatic pointer forwarding from the attached HTML canvas.

The Render Worker calls `configure_wasm_workers` once, then on each `load`
sizes its `OffscreenCanvas` to `FrameSize::for_viewport` and builds a
`LynxView` over it — the canvas is a construction argument, because the view
builds its surface before it exists. That core API configures the worker
bootstrap used by both the engine-owned Lynx main task and the private Stylo
Rayon pool. The Wasm embedder does not take a document owner or initialize
Stylo itself. One Wasm instance owns one `BobcatRenderer` and one configured
pool; that renderer owns a sequence of non-overlapping native views, none
until the first load, each later load dropping the current view. Core
explicitly stops and joins the per-view Lynx-main Worker after it drops its
document and thread-bound realm; only then can replacement construction
begin. The
process-wide pool adopts the persistent Render Worker as index zero and
remains valid across loads. The configured count covers that owner plus at
least one managed Stylo Worker; each live view's Lynx-main Worker is separate.
The public facade still creates one fresh Render Worker and Wasm instance per
`BobcatCanvas`, not per load.

The transient Lynx-main Worker invokes Stylo from outside its Rayon pool, so
Stylo transfers that traversal's root closure onto a managed worker. The Render
Worker enters from its stable index-zero slot. This keeps view lifetime
independent from the shared pool without requiring an idle script Worker to
service style work.

## Resource and script boundaries

Browser fetch policy stays in JavaScript. For `load(url, styleSheetUrls)`, the
Render Worker uses browser `fetch`, reads each response stream with a 16 MiB
bound, and registers the raw stylesheet and entry-MTS bytes in its
`BrowserResources` under their final response URLs, then calls
`BobcatRenderer::load(entry_url, style_sheet_urls)`. Core reads them through
`ResourceFetcher`, validates UTF-8 strictly, and uses the entry's final URL as
the ESM entry specifier; it never receives a bundle decoder. That 16 MiB bound
is browser-embedder policy and does not cross in `ResourceRequest`.

`loadLynxXml(url)` similarly fetches the source envelope once and decodes it
with the browser's replacement-mode UTF-8 `TextDecoder`, matching web-core's
raw XML loader. Rust's `lynx-xml` parser validates and extracts the sections in
the Render Worker. The source uses `<lynx engine-version="...">` and
`<script thread="main">` / `<script thread="background">`; legacy
attribute spellings are rejected. A present stylesheet is registered as CSS
text and mounted as the view's one sheet before the main-thread section starts;
the returned Promise uses the same engine-event completion path as `load`. The
exported `LYNX_XML_PAGE_CONFIG` supplies the source format's fixed
`false`/`false`/`true` display/overflow/selector defaults, while callers may
still pass an intentional host override to `BobcatCanvas.create`.

Both entry points are repeatable: a page's sources are its view's construction
inputs, so each call builds a fresh view and drops the one before it rather
than mutating a running page. Every source is fetched and registered before
that replacement, so a load that cannot fetch leaves the running page and its
cascade untouched. The outer Worker, OffscreenCanvas, Wasm module, page
configuration, latest device metrics, resource provider, font containers, and
default font family are the renderer's own and are reapplied to each view it
builds; the view, Lynx-main Worker, VM, and document are replaced. The
provider clears the registered bytes once construction has copied them, so
repeated Blob-URL submissions do not accumulate stale sources.

The optional XML background section is reported under a URL derived from the
final XML response URL, but neither retained nor executed: Bobcat has no
background-thread realm yet, and says so explicitly.

The UI facade resolves relative script, stylesheet, and Lynx XML URLs against
the embedding document's `document.baseURI` before crossing the Worker boundary.
The Render Worker accepts only absolute URLs: resolving there against
`self.location` would incorrectly use the npm package/Worker URL as the base.

The browser names no script engine at all. Core creates its realm inside the
Lynx main Worker, so the realm remains owner-thread-bound and uses Bobcat's
primitive-only host callbacks. Raw QuickJS values, realm handles, numeric DOM
ids, and host callbacks are not surfaced by the npm facade.

Startup is an asynchronous host boundary whose owned work runs on the Lynx
main Worker. It creates the document, awaits core resource futures, mounts the
stylesheets, then creates QuickJS and preloads `bobcat:runtime`,
`bobcat:element`, and the resolved entry URL;
the `bobcat:boot` module uses top-level await to import the entry before it
calls a present `globalThis.renderPage` or dispatches `__RenderPage` on the
realm-local EventTarget returned by `lynx.getEngine()`. It then flushes the
element tree. QuickJS drains its owned pending-job queue until that
module-evaluation promise settles. `LynxView::new` resolves only after that
boot succeeds and rejects on any resource, font, realm, or boot failure, so a
failed startup never exposes a half-initialized view. `ScriptFinished` remains
queued as the successful lifecycle edge consumed by the browser loop. No
browser microtask checkpoint or timer interception participates in completion;
the fallback listener is retained inside the preloaded runtime ESM rather than
by the browser.

`ListenerFailed` is written to the browser console without stopping the page,
while a later script-Worker failure remains fatal; neither is tied to animation
frames, so a hidden document cannot strand it. A load advances the loop's
generation before replacing the native view, so an old page cannot consume the
new page's event.

There is no browser create/append/drop/flush/direct-stylesheet API. Element
mutation is reachable only from the fetched entry MTS module through the named
exports of `bobcat:element`. `registerFonts(bytes)` and
`setDefaultFontFamily(family)` retain wrapper state: faces are registered, and
the family checked against them, when a view is built, so both must precede a
load, and a family nothing provides makes that load reject. Author stylesheets
reach core the way the entry module does — fetched and registered by the Render
Worker, named in the load, mounted as author-origin rules in cascade order. The
stylesheet contract has a second arm for a host that already parsed its CSS,
but a browser host never does, so this embedder always takes the text arm.
The browser registry is one normalized-URL-to-bytes map. Script and stylesheet
registration populate that same map; `fetch_resource` and `fetch_style_sheet`
decide how the selected bytes are interpreted.

The browser facade still does not decode `.web.bundle` containers. A caller
may load suitable JavaScript by URL or a raw Lynx XML source card;
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
shares the same ordered Worker queue as load and resize, so it cannot re-enter
the Wasm wrapper while an asynchronous view replacement owns it and a pointer
following resize is interpreted in the metrics installed before it.

No timestamp crosses the seam. `BobcatRenderer::dispatchPointer` constructs
core's `InputEvent` and calls the opaque `LynxView::dispatch_input`, which
stamps the event's arrival from the engine's own clock — the same clock its
frames read — so a press after a long idle period cannot derive its `longpress`
deadline from the last rendered frame, and nothing has to agree on a time
origin. Each load releases active captures before replacing the view; disposal
stops input before terminating the Worker. Wheel input is not connected yet.

## Synchronization and rendering

The private document lives on the Lynx main Worker outright; commits publish
an immutable frame the Render Worker composes, and changes travel the other
way as ordered commands. A JavaScript turn therefore cannot expose partial
mutation or stall the last published frame. One lost-wake-safe event signal
carries everything back from the Lynx main Worker: it wakes a Promise whenever
core queues an engine event *or* wants a frame drawn, and the Render Worker's
loop answers each wakeup with one `pump` — draw the pending frame, drain the
events. The same signal is what this Worker arms for *itself*, because the
view paints here and wakes nobody on its own: a pointer or a resize that
arrives while the loop is parked applies immediately and then arms the signal
so the turn it owes actually happens, and a swap chain that had no image to
give arms it again after the delay `LynxView::next_turn` names, through a
Worker timer, so the frame is retried rather than dropped and the retry does
not spin the loop through microtasks. No frame clock stands between a commit and the canvas. The clock is
the continuation's alone: while `isAnimating` reports that the engine owes the
timeline another frame, the loop waits for the next display frame instead —
`requestAnimationFrame` where a Worker is given one, a frame-interval timer
where it is not — because drawing faster than the compositor shows is waste.
An animation therefore crosses nothing. `pump` takes no argument: the animation
timeline is core's own `web_time` clock, read once per frame on the Render
Worker after the canvas surface hands over an image. `requestAnimationFrame`'s
`DOMHighResTimeStamp` would be taken on the page's main thread, before this
Worker is woken and on a different time origin than its `performance.now()`,
so it is not the better reading it appears to be. Script completion therefore
does not poll and does not depend on a frame clock, so a hidden page cannot
strand a `load` Promise merely by suspending drawing. The UI facade, nested VM Worker startup, and built-in
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
import, the facade's four page and font declarations and their dispatches, that
a load registers a page's sources before building the view, and the absence of
the private pointer method and the removed direct DOM API.

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
canvas only for the first render, registering its font container and default
family once. Every submit is one `loadLynxXml` on that warm canvas, rebuilding
only the native `LynxView`; the Render Worker, OffscreenCanvas, Wasm instance,
Stylo pool, and retained font bytes stay. The Blob URL is
revoked only after `loadLynxXml` settles.

Synchronous GPU readback remains absent because browser WebGPU map completion
is Promise-driven; native capture blocks on device polling. Browser capture
requires a separate asynchronous facade API.
