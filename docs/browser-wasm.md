# Browser Wasm embedder

`crates/bobcat-wasm` is the `wasm-bindgen` browser embedder and npm facade for
`wasm32-unknown-unknown`. It builds with shared memory and uses crates.io
Vello 0.9/wgpu 29. The browser build enables Bobcat's QuickJS feature and uses
the same built-in `ScriptEngineFactory` as native Bobcat.

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

The browser passes `quickjs_engine_factory()` directly to `LynxView`. Core
moves the transferable factory into the Lynx main Worker and calls `create`
there, so the resulting realm remains owner-thread-bound and uses Bobcat's
primitive-only host callbacks. Raw QuickJS values, realm handles, numeric DOM
ids, and host callbacks are not surfaced by the npm facade.

Entry evaluation is synchronous. QuickJS drains its owned pending-job queue at
each execution checkpoint before returning to core, so script completion is
exactly the `ScriptFinished` engine event. No browser microtask checkpoint,
timer interception, or JavaScript callback-retention protocol participates in
completion.

There is no browser create/append/drop/flush/direct-stylesheet API. Element
mutation is reachable only from the fetched main-thread script through the
embedded Element PAPI. `registerFonts(bytes)` is a narrow resource capability:
it registers every usable face in an OpenType container through the opaque
view and returns the number accepted, without exposing the document or text
engine. `loadStyleSheet(url)` fetches CSS in the Render Worker, registers the bytes,
and mounts them as author-origin rules through the same resource boundary the
main-thread script uses; sheets cascade in load order. The stylesheet contract
has a second arm for a host that already parsed its CSS, but a browser host
never does, so this embedder always takes the text arm.

The browser facade still does not decode `.web.bundle` containers. A caller
may execute suitable JavaScript by URL; bundle retrieval, decode, `PageConfig`
parsing, and `StyleInfo` lowering remain external work, exactly as in the
native CLI — where the CLI does perform them and hands core the pre-parsed
arm.

## Synchronization and rendering

The private document moves through core's `SharedTree` slot. A PAPI batch
takes the document; the Render Worker only tries a non-blocking borrow. An
open batch therefore cannot expose partial mutation or stall the last retained
frame. A shared atomic `FrameSignal` carries redraw requests from the Lynx main
Worker to the Render Worker, whose animation loop calls `renderIfRequested`.
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
import, the URL execution methods, and the absence of the removed direct DOM
API.

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
