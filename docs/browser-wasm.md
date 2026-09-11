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
  owns one Painter over the transferred canvas, for its whole life
  owns the opaque LynxView of the current page + the resource registry
  owns Vello/wgpu/OffscreenCanvas
  └── core wasm_thread spawn, two Workers per LynxGroup
      ├── Lynx main/VM Worker (bobcat-main)
      │   ├── owner-thread-bound QuickJS realm per view
      │   ├── the realm's own document + Element PAPI
      │   ├── index 0 of its group's Stylo pool, whose
      │   │   other members it wasm_thread-spawns
      │   └── one message sender on the worker-realm Worker
      └── worker-realm Worker (bobcat-workers)
          └── its own QuickJS runtime: one realm and one task
              per live Worker of any view in the group
```

**The painter outlives the pages.** `BobcatRenderer` builds one `Painter` over
the transferred canvas at `create` and keeps it across loads, rebuilding it
only when it is missing or its draw target has failed. A `load` is
`painter.detach()` → drop the previous view → drop its group, which ends that
group's Lynx-main Worker and then its worker-realm one → build the new group
and view → `painter.attach(&view)`. The
canvas is deliberately *not* resized along the way: it already carries the
right resolution, and setting a canvas's size clears its bitmap, which would
blank the outgoing page's last frame while the next one boots. Detaching
leaves that frame on screen and still capturable; attaching imposes the
painter's metrics on the new view, so a page built at the wrapper's size lays
out at the canvas's.

The UI thread never instantiates Wasm and never owns an engine, document,
tree, scene, GPU object, or Rust session registry. Its public operations are
limited to canvas creation with `PageConfig`, URL-based page loads, font and
default-family registration, resize, error observation, disposal, and
automatic pointer forwarding from the attached HTML canvas.

The Render Worker calls `configure_wasm_workers` once, then sizes its
`OffscreenCanvas` to `FrameSize::for_viewport` before building its `Painter`
over it — a host that owns the surface's backing store has to size it before
it hands the target over. That core API configures the worker
bootstrap used by the engine-owned Lynx main Worker, the group's worker-realm
Worker, the wasm32 alarm Worker, and the group's Stylo
Rayon pool. The Wasm embedder does not take a document owner or initialize
Stylo itself. One Wasm instance owns one `BobcatRenderer`, one painter, and a
sequence of non-overlapping groups and views — none
until the first load, each later load dropping the current pair. Dropping the
view ends its task, which releases the realm, the document that realm created,
and every worker it made; dropping the group ends the Lynx-main Worker and,
after it, the worker-realm Worker whose only remaining sender the group holds,
and only then does replacement construction begin. Ending is all it is here:
under this target's `panic=abort` a trapped Worker never signals its join
handle, so wasm teardown says the goodbye and does not wait.
The public facade still creates one fresh Render Worker and Wasm instance per
`BobcatCanvas`, not per load.

The Render Worker is not a member of any style pool. Each group's Lynx-main
Worker is index zero of the pool it builds, taken over in place by rayon's
`use_current_thread`, and the count `BobcatRenderer::create` is given includes
it — so the facade asks for the machine's threads less the Render Worker. A
pool retires with the group that built it.

## Resource and script boundaries

Page sources keep their browser fetch policy in JavaScript. For
`load(url, styleSheetUrls)`, the Render Worker uses browser `fetch`, reads
each response stream with a 16 MiB bound, and registers the raw stylesheet and
entry-MTS bytes with the `bobcat-resources` system it owns
(`BobcatRenderer` holds one `Resources` for its whole life) under their final
response URLs, then calls `BobcatRenderer::load(entry_url, style_sheet_urls)`.
Core reads them through `ResourceFetcher`, validates UTF-8 strictly, and uses
the entry's final URL as the ESM entry specifier; it never receives a bundle
decoder. That 16 MiB bound is browser-embedder policy and crosses no part of the
resource protocol. The load also makes the entry URL the base a relative
`url(...)` in the page's CSS resolves against.

Everything else a page names — its images — the resource system fetches
itself, in Rust, through the same Worker `fetch` (so the same origin, CORS,
credentials and HTTP-cache policy apply), and decodes with the platform's
`Image` element on the main thread: `image-decoder.js`, shipped in the package
beside `facade.js`, which the facade connects to the Render Worker over a
`MessageChannel` at init. The Render Worker hands the fetched bytes over; the
main thread turns them into a Blob URL, decodes and resizes them through a 2D
canvas, and copies the RGBA pixels straight into a buffer the Render Worker
allocated in the shared Wasm memory, where each decode job has a small
mailbox of eight `Int32` words. An ordinary load completes on the event loop
and wakes the page loop
through the engine signal; a restore after eviction is the one call that
blocks the Render Worker, with `Atomics.wait` on that job mailbox, because a
read after a reported load must not miss. That read happens where the painter
adopts a commit, which every painter entry point runs before the drawing path
acquires a swap-chain image, so a restore cannot stall the chain. The main thread never waits, which is
what keeps the two from deadlocking, and its only cost per image is the pixel
read-back. The resource system's diagnostics — an image that failed, a missing
decoder — reach `console.warn`.

`loadLynxXml(url)` similarly fetches the source envelope once and decodes it
with the browser's replacement-mode UTF-8 `TextDecoder`, matching web-core's
raw XML loader. Rust's `bobcat-source::xml` parser validates and extracts the sections in
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
provider releases the current page's registered scripts and styles after its
startup outcome arrives, so repeated Blob-URL submissions do not accumulate
stale sources. Other page assets remain available until that page is retired.

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
main Worker. The view's task there validates fonts, requests its sources
through the view's own channel, and stages what arrives; it then creates a
QuickJS realm, preloads `bobcat:runtime`, `bobcat:element`, the timer and
event-target modules, and the resolved entry URL, and evaluates
`bobcat:boot`. That module's first statement constructs its `Document`, which
is what builds the page out of the staged ingredients — the author sheets
mounted in cascade order among them. It then uses top-level await to import
the entry before it
calls a present `globalThis.renderPage` or dispatches `__RenderPage` on the
realm-local EventTarget returned by `lynx.getEngine()`, and finally flushes the
element tree. QuickJS drains its owned pending-job queue at each turn.
Dynamic imports request JavaScript modules through the same asynchronous
resource completions; unresolved imports and top-level await retain the boot
promise while the Render Worker goes on pumping the view. A timer deadline is
not the Worker's to wait out — the view's own task waits it out, through the
`bobcat-alarm` Worker on this target, and the commit that follows arms the
engine signal like any other publication. `LynxGroup::create_lynx_view` is
synchronous and returns a loading view. Normal `LynxView::pump` turns report
`ScriptFinished`
after boot succeeds or `StartupFailed` on resource, font, realm, or boot failure;
the browser load promise waits for that lifecycle outcome. No
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
stylesheet contract has a second arm for pre-parsed CSS. Raw JavaScript and
XML loads take the text arm; `loadTemplate` and `loadZip` decode binary containers
through `bobcat-source::PageSource` and register their lowered `StyleInfo` through
the pre-parsed arm. Source retrieval and adaptation remain embedder work,
with core receiving only `ViewSources` and its resource-fetcher contract.

`request_source` launches a browser task that resolves, fetches and validates
the requested source, then uses its concrete completion handle to answer the
task awaiting it directly. Neither turn polls source IO. Each page owns an independent
resource scope: boot scripts and styles remain registered until its startup
outcome arrives; ZIP images and other assets remain available for later frames.
Sources staged for the next page belong to a separate scope and are not cleared
when the current page completes.

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
core's `InputEvent` and calls the canvas painter's `dispatch_input`, which
stamps the event's arrival from the engine's own clock — the same clock its
frames read — so a press after a long idle period cannot derive its `longpress`
deadline from the last rendered frame, and nothing has to agree on a time
origin. Each load releases active captures before replacing the view; disposal
stops input before terminating the Worker. Wheel input is not connected yet.

## Synchronization and rendering

The private document lives on the Lynx main Worker outright, owned by the
realm that created it; commits publish
an immutable frame the Render Worker composes, and changes travel the other
way as ordered commands. A JavaScript turn therefore cannot expose partial
mutation or stall the last published frame. One lost-wake-safe event signal
carries everything back from the Lynx main Worker: it wakes a Promise whenever
core queues an engine event *or* wants a frame drawn, and the Render Worker's
loop answers each wakeup with one `BobcatRenderer::pump`. That stays one
method and takes both turns in order — `Painter::pump` draws the frame the
canvas painter owes, then `LynxView::pump` services the host's resources and
hands back the lifecycle events. The painter goes first deliberately: the
pixels a fatal script error left behind reach the canvas on the turn that
reports it, with nobody left to ask for another frame.
The same signal is what this Worker arms for *itself*, because the
painter draws here and wakes nobody on its own: a pointer or a resize that
arrives while the loop is parked applies immediately and then arms the signal
so the turn it owes actually happens. A frame the turn leaves owed is not
armed at all — `owesFrame()` says so, and the loop takes it at the next
display frame instead, which is `requestAnimationFrame` where a Worker is
given one. No frame clock stands between a commit and the canvas. The clock is
the continuation's alone: while `owesFrame()` reports that the painter still
has a frame to put on the canvas, the loop waits for the next display frame
instead of the engine signal —
`requestAnimationFrame` where a Worker is given one, a frame-interval timer
where it is not — because drawing faster than the compositor shows is waste. A
realm timer is not this loop's to wait out either: the engine waits its own
out and arms this signal when the entry it ran commits.
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
only the native `LynxGroup` and its one `LynxView`; the Render Worker,
OffscreenCanvas, Wasm instance, Stylo worker count, and retained font bytes
stay — the Stylo workers themselves belong to the group and are rebuilt with
it. The Blob URL is
revoked only after `loadLynxXml` settles.

Synchronous GPU readback remains absent because browser WebGPU map completion
is Promise-driven; native capture blocks on device polling. Browser capture
requires a separate asynchronous facade API.

Source mapping and registration live in the complete `bobcat-source` crate,
including its XML, web and native binary parsers, with no Cargo feature flags.
The `loadLynxXml` API continues to accept XML responses only.
The one-shot response adapter preserves final-URL fragments and reports
background presence without registering its body. Host PageConfig stays authoritative.

`loadTemplate(url)` fetches a binary web or source-based native bundle with a
16 MiB response bound and delegates decoding, configuration and StyleInfo
registration to `PageSource`. Native bytecode and missing root entries retain
the shared parser's errors. The renderer uses the input URL as the resource
base and the bundle configuration for that view; subsequent raw XML loads
still use the host configuration.

Both Pages tabs use the same two-column workspace. Canvas provides a local
ZIP picker and an entry field: a ZIP-root-relative path, or an HTTP(S) URL whose
decoded pathname exactly matches an archive member. Clicking **Load template**
passes the ZIP bytes and absolute entry URL to `loadZip`. Relative paths receive
a `bobcat-memory://archive/` base. The Render Worker delegates decoding,
entry selection and resource registration to the platform-independent
`bobcat-source::ZipSource`; Pages contains no ZIP parser or storage layer.
The COOP/COEP service worker remains responsible only for browser isolation.

`ZipSource::page` applies `PageSource` to the selected member (strict UTF-8 XML,
web binary, or source-based native bundle with a root entry). `register_with`
moves the files into `bobcat-resources` under the entry URL's origin and their
ZIP-root paths. Relative resources resolve from the entry's directory; matching
absolute URLs also use the registry, while other URLs follow the embedder's
ordinary transport policy. The shared decoder enforces 64 MiB compressed,
128 MiB actual decompressed output and 4096 entries, rejecting unsafe paths,
duplicates and symlinks. Stored/deflated, single-disk ZIPs are supported;
ZIP64 archive directories are outside this bounded package format.

Each browser page receives a separate `Resources::new_scope()`. Registrations
stay alive for delayed image loads, and image caches and completion queues
cannot leak an old ZIP's images into a replacement using the same paths. IO
workers, the browser image decoder and native disk cache remain shared. The
previous page stays usable if ZIP or template parsing fails before replacement;
retiring a view releases its scope once outstanding resource work completes.
ZIP tests live in bobcat-source and run in native CI and the existing Wasm test
step. The Pages deployment workflow builds and publishes the site.

The **Expand** button sits in the preview heading outside the canvas and hides
the active source panel. **Restore** or Escape returns to the split layout,
preserving the selected ZIP, entry text and XML edits. Both tabs share the
expanded state, and the existing ResizeObserver resizes the warm canvas.
Narrow screens stack the source and preview panels vertically.
