# Browser Wasm embedder

`crates/bobcat-wasm` is the `wasm-bindgen` browser embedder and npm facade for
`wasm32-unknown-unknown`. It builds with shared memory and uses crates.io
Vello 0.9/wgpu 29. The browser build disables Bobcat's QuickJS feature and
injects a browser JavaScript VM through the same `ScriptEngineFactory`
contract used by native QuickJS.

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
                               ├── browser ScriptEngine
                               ├── Element PAPI + private document batches
                               └── core wasm_thread spawn -> Stylo workers
```

The UI thread never instantiates Wasm and never owns an engine, document,
tree, scene, GPU object, or Rust session registry. Its public operations are
limited to view creation with `PageConfig`, URL-based script and stylesheet
requests, font registration, resize, error observation, and disposal.

The Render Worker constructs `LynxView`, attaches the transferred canvas, and
calls `configure_wasm_workers`. That core API configures the worker bootstrap
used by both the engine-owned Lynx main task and the private Stylo Rayon pool.
The Wasm embedder does not take a document owner or initialize Stylo itself.
Because the pool includes its Lynx-main owner thread, one Wasm instance owns
one renderer. The public facade creates one fresh Render Worker and Wasm
instance per `BobcatCanvas`. A minimum pool size of two leaves one managed
Rayon worker available after the synchronous entry-task Worker exits.

## Resource and script boundaries

Browser fetch policy stays in JavaScript. For `executeScript(url)`, the Render
Worker uses browser `fetch`, reads the response stream with a 16 MiB bound,
and registers the raw response bytes in its
`BrowserResources` implementation under the final response URL, then calls
the opaque Rust view's `execute_script(url)`. The view resolves and reads the
bytes through `ResourceFetcher` and performs strict UTF-8 validation; it never
receives a bundle decoder.

The UI facade resolves relative script and stylesheet URLs against the
embedding document's `document.baseURI` before crossing the Worker boundary.
The Render Worker accepts only absolute URLs: resolving there against
`self.location` would incorrectly use the npm package/Worker URL as the base.

The browser `ScriptEngineFactory` is a transferable capability that retains
only shared lifecycle atomics and the engine-event signal, never a realm,
tree, or document. Core moves it into the Lynx main Worker and calls `create`
there. Its owner-thread-bound `BrowserScriptEngine` uses the Worker's
JavaScript global, installs Bobcat's primitive-only host callbacks, evaluates
named source, and maps JavaScript failures into sanitized `ScriptError`
values. Raw JS values, realm handles, numeric DOM ids, and host callbacks are
not surfaced by the npm facade.

The current browser VM is a synchronous entry-script adapter. Promise
microtasks queued by a synchronous `renderPage` boot are allowed one browser
checkpoint before the VM Worker completes: host dispatch closures remain live
through that checkpoint. Promise-deferred installation of `renderPage` is
still not supported because browsers expose no synchronous microtask-drain API
between app evaluation and Bobcat's boot evaluation. QuickJS retains its
ordinary owned-job checkpoints.

There is no browser create/append/drop/flush/direct-stylesheet API. Element
mutation is reachable only from the fetched main-thread script through the
embedded Element PAPI. `registerFonts(bytes)` is a narrow resource capability:
it registers every usable face in an OpenType container through the opaque
view and returns the number accepted, without exposing the document or text
engine. `loadStyleSheet(url)` currently forwards to core and rejects as
unsupported without fetching.

The browser facade still does not decode `.web.bundle` containers. A caller
may execute suitable JavaScript by URL; bundle retrieval, decode, `PageConfig`
parsing, and future `StyleInfo` lowering remain external work, exactly as in
the native CLI.

## Synchronization and rendering

The private document moves through core's `SharedTree` slot. A PAPI batch
takes the document; the Render Worker only tries a non-blocking borrow. An
open batch therefore cannot expose partial mutation or stall the last retained
frame. A shared atomic `FrameSignal` carries redraw requests from the Lynx main
Worker to the Render Worker, whose animation loop calls `renderIfRequested`.
A separate lost-wake-safe event signal wakes a Promise whenever core queues an
engine event or the browser VM reaches its final microtask checkpoint. Script
completion therefore does not poll and does not depend on rAF, so a hidden
page cannot strand an `executeScript` Promise merely by suspending drawing.
The startup handshake has a ten-second deadline until the nested VM Worker
starts. After that point there is deliberately no execution deadline: the
browser-injected VM has no safe interrupt API, so an infinite script leaves
`executeScript` pending. Dispose the `BobcatCanvas` and create a replacement
canvas/Worker to recover. This limitation is specific to the browser adapter;
native QuickJS embedders may apply a different timeout or interrupt policy.

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

The package invokes `wasm-pack` for the `web` target. Generated glue and Wasm
live under `crates/bobcat-wasm/pkg/` and are not checked in. The verification
script checks shared imported/exported memory, the Worker-only Wasm import,
the URL execution methods, and the absence of the removed direct DOM API.

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

Synchronous GPU readback remains absent because browser WebGPU map completion
is Promise-driven; native capture blocks on device polling. Browser capture
requires a separate asynchronous facade API.
