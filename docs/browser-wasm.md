# Browser Wasm embedder

`crates/bobcat-wasm` is a pure-Rust browser embedder and pnpm package built
with `wasm-bindgen`/`wasm-pack` for `wasm32-unknown-unknown`. It uses the
crates.io releases of Vello 0.9 and wgpu 29; no NAPI, WASI, or private
Vello/wgpu fork participates in the browser build.

The generated module uses shared `WebAssembly.Memory`. Every Wasm instance in
the browser topology is initialized from the same compiled module and memory,
so the Rust `SharedTree`, command queues, retained scene, and synchronization
state remain in one address space.

## Execution and ownership

The browser has three permanent roles:

```text
browser UI thread
  JavaScript Promise facade only
  └── creates an explicit embedder Worker and transfers OffscreenCanvas

embedder/Render Worker
  initializes shared Wasm
  constructs the complete Bobcat Engine
  ├── retained Vello scene -> wgpu 29 -> OffscreenCanvas
  └── wasm_thread -> nested Lynx main/DOM Worker
                         ├── batch mutations -> Stylo/Rayon -> layout
                         └── wasm_thread -> Stylo Rayon Workers
```

The browser UI thread never owns an `Engine`, wgpu `Device`, `Queue`,
`Surface`, Vello renderer, canvas rendering context, Wasm instance, or Rust
session registry. It also never waits on a Rust mutex or joins a thread. Public
host operations cross into the embedder Worker as Promise-backed messages; the
worker then sends DOM work over `std::sync::mpsc` in shared Wasm memory.

The embedder Worker constructs `Engine::new`, attaches the transferred canvas,
and takes the engine-created unique main-thread document owner. It moves that
owner into a nested module Worker through `wasm_thread`; it never constructs a
tree on the UI thread or asks a renderer to adopt somebody else's tree. All
WebGPU objects are created and used permanently on the embedder Worker.
Creation resolves only after a shared-atomic startup handshake from that nested
Worker; an asynchronous deadline turns script/import/bootstrap failure into
initialization failure without blocking the embedder Worker or entering a
permanent join.

The DOM side takes the document from `SharedTree` for a mutation batch and
returns it at commit. The Render Worker borrows the slot non-blockingly; an
open batch therefore cannot stall animation or presentation and cannot expose
a half-applied tree. The DOM Worker constructs Stylo's normal private
`rayon::ThreadPool`, includes itself as worker zero, and uses `wasm_thread` to
start the remaining browser Workers in the same memory. Bobcat injects that
pool through Stylo's existing `traverse_dom(..., Some(pool))` path; no Stylo
source modification or global-pool override is involved.

DOM responses wake an async Rust shared-memory signal and are pumped
independently of Worker animation frames. Background-page rAF suspension can
therefore pause drawing without stranding host-operation Promises; this is a
control-plane wakeup, not a serialized DOM or reconciliation channel.

The crates.io `wasm_thread` 0.3.3 release forwards a spawn made inside any
Worker to a private parent callback. An explicitly-created embedder Worker does
not have that callback. The workspace therefore pins the first commit of the
upstream `spawn_from_worker` change, which directly creates the nested Worker.
This changes only thread bootstrap; DOM/render state still crosses exclusively
through Rust synchronization. Nested dedicated Workers are available from
Chrome 69 and nested module Workers from Chrome 80, below this package's Chrome
135 floor.

The package currently builds `bobcat-core` without QuickJS and exposes the
implemented direct DOM/Element-PAPI command surface. Porting the owner-thread
QuickJS C runtime to `wasm32-unknown-unknown` is separate unfinished work.
Consequently the browser facade does **not** claim execution of compiled
ReactLynx `.web.bundle` files yet.

## Build and isolation

Build the browser bindings with:

```sh
pnpm --filter bobcat-wasm build
```

The package invokes `wasm-pack` for the `web` target. The pinned nightly and
`rust-src` component are required because threaded `std` is rebuilt with Wasm
atomics; `.cargo/config.toml` enables shared memory plus the Chrome-135-safe
code-generation features (SIMD/relaxed SIMD, bulk memory, multivalue,
non-trapping conversions, sign extension, reference types, extended constants,
and tail calls). Generated bindings and the `.wasm` binary live under
`crates/bobcat-wasm/pkg/` and are not checked in.

The browser crate also enables `parking_lot_core/nightly`. Without that
feature, parking_lot deliberately selects its non-atomic generic Wasm parker,
which panics as soon as a contended Stylo/wgpu lock needs to sleep; with the
feature and `+atomics`, it uses Wasm atomic wait/notify like the standard
library synchronization used elsewhere in the engine.

The workspace pins `nightly-2026-04-20` in `rust-toolchain.toml`. A later
`build-std` snapshot using
`dlmalloc` 0.2.13 overlaps wasm-bindgen's injected thread-bootstrap page: the
first TLS allocation overwrites the Worker stack lock and leaves every
secondary instance spinning during `__wbindgen_start`. Changing this browser
pin therefore requires a real cross-origin-isolated browser smoke test in
addition to the compile-time shared-memory checks.

Shared memory, Wasm threads, and `wasm_thread` require a cross-origin-isolated
page. `packages/github-pages/public/coi-service-worker.js` adds these headers
to same-origin responses and the page reloads once after the worker takes
control:

```text
Cross-Origin-Opener-Policy: same-origin
Cross-Origin-Embedder-Policy: require-corp
```

With `require-corp`, remote images, fonts, scripts, or bundles must use CORS or
a compatible Cross-Origin-Resource-Policy. The demo keeps its Wasm, Worker,
script, and stylesheet assets on the Pages origin.

Primary references:

- [`Rayon` custom thread pools](https://docs.rs/rayon/latest/rayon/struct.ThreadPoolBuilder.html)
- [`wasm_thread`](https://crates.io/crates/wasm_thread)
- [`wasm_thread` nested-Worker change](https://github.com/chemicstry/wasm_thread/pull/34)
- [wgpu Web/Wasm support](https://github.com/gfx-rs/wgpu)
- [Vello](https://github.com/linebender/vello)
- [COOP](https://developer.mozilla.org/en-US/docs/Web/HTTP/Reference/Headers/Cross-Origin-Opener-Policy) and [COEP](https://developer.mozilla.org/en-US/docs/Web/HTTP/Reference/Headers/Cross-Origin-Embedder-Policy)

Synchronous GPU readback remains intentionally absent: browser WebGPU map
completion is Promise-driven, while the native screenshot path blocks on
device polling. Browser capture needs a separate asynchronous API.
