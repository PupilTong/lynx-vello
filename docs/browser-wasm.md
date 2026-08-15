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
limited to view creation with `PageConfig`, `executeScript(url)`, the reserved
`loadStyleSheet(url)`, resize, error observation, and disposal.

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

The browser `ScriptEngineFactory` is a transferable, zero-state capability.
Core moves it into the Lynx main Worker and calls `create` there. Its
owner-thread-bound `BrowserScriptEngine` uses the Worker's JavaScript global,
installs Bobcat's primitive-only host callbacks, evaluates named source, and
maps JavaScript failures into sanitized `ScriptError` values. Raw JS values,
realm handles, numeric DOM ids, and host callbacks are not surfaced by the
npm facade.

The current browser VM is a synchronous entry-script adapter. Browsers expose
no synchronous microtask-drain API between app evaluation and Bobcat's boot
evaluation, so Promise-deferred installation of `renderPage` is not supported
yet. QuickJS retains its ordinary owned-job checkpoints.

There is no browser create/append/drop/flush/register-font/direct-stylesheet
API. Element mutation is reachable only from the fetched main-thread script
through the embedded Element PAPI. `loadStyleSheet(url)` currently forwards to
core and rejects as unsupported without fetching.

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
Script completion is polled on a separate serialized control-plane timer, so
hidden-page rAF suspension cannot strand an `executeScript` Promise. That
Promise resolves after boot and rejects on fetch, VM initialization, or
evaluation failure.

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
