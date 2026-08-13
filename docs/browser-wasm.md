# Browser Wasm embedder

`crates/bobcat-wasm` is one pure-Rust source crate and one pnpm package. It
produces one threaded browser artifact:

| Rust target | Host ABI | Responsibilities |
| --- | --- | --- |
| `wasm32-wasip1-threads` | NAPI-RS + Emnapi/WASI | `std::thread`, atomics and blocking synchronization; Bobcat `Engine`; Vello/wgpu HTML Canvas presentation |

The NAPI-RS loader instantiates shared `WebAssembly.Memory` and supplies the
WASI thread and async-work Workers. The vendored wgpu branch adds a Node-API
WebGPU bridge for WASIP1, so the same compiled module can adopt an HTML canvas,
create a wgpu surface, and run Vello directly in the threaded artifact. Vello
itself remains a normal wgpu backend consumer.

This is one `.wasm` artifact with one Rust address space shared by its WASI
thread instances. Rust-owned engine state, the retained `vello::Scene`, and
Element-PAPI objects therefore no longer cross an ABI boundary between two
independent modules.

## Execution and ownership

The current threaded export is `parallelChecksum(bytes, threads)`. Its NAPI
`AsyncTask` runs on an Emnapi worker, creates scoped Rust threads, atomically
combines their results, synchronously joins them, and resolves a Promise on the
JavaScript thread. It is an end-to-end probe of real threads, atomics, and
blocking synchronization rather than rendering policy.

The module also owns `BobcatCanvas` and a `bobcat-core::engine::Engine` with
QuickJS disabled. It exposes asynchronous WebGPU initialization, CSS
viewport/DPR resizing, author styles and fonts, the five implemented Element
PAPI mutations, and requested-frame presentation. The document creates one
retained Vello scene and the engine submits it directly to wgpu's Canvas
surface.

One Wasm module does not make browser GPU objects freely movable between
threads. The Canvas plus wgpu Device, Queue, Surface, and Vello renderer are
thread-affine and remain on the browser UI thread. CPU-only NAPI async work may
block and spawn Rust threads, but those workers must not own or call the WebGPU
objects.

The built-in QuickJS realm is owner-thread-bound too. It must not be placed
inside arbitrary NAPI `AsyncTask`s because successive tasks can run on
different pool workers. The intended ownership is:

```text
browser UI thread
  Bobcat Engine -> retained Vello scene -> wgpu Canvas presentation

NAPI async-work / WASI Rust workers
  CPU-only work, std::thread, atomic wait/notify, blocking joins

future dedicated WASI Rust thread
  permanently owns the QuickJS realm and its event loop
```

The single-module design removes the former cross-ABI module boundary, but the
dedicated QuickJS thread has not been connected to the UI-thread engine yet.
The Pages demo drives the implemented Element PAPI directly and does not claim
browser execution of a compiled ReactLynx `.web.bundle`.

## Build and isolation

The package has one Rust generator:

```sh
pnpm --filter bobcat-wasm build:wasi
```

`napi build --platform --release --target wasm32-wasip1-threads` keeps the
generated `.wasm`, browser loader, and workers together. The loader uses
top-level await, ESM workers, and `import.meta.url`, all of which the Rsbuild
package must preserve. A checked post-build transform caps its eagerly-created
pthread pool at three Workers; NAPI-RS also keeps its default four async-work
Workers. Each exported CPU operation clamps its own Rust thread count to three
as well.

The threaded loader and `SharedArrayBuffer` require a cross-origin-isolated
page. `packages/github-pages/public/coi-service-worker.js` adds these headers to
same-origin responses and the page reloads once after the worker takes control:

```text
Cross-Origin-Opener-Policy: same-origin
Cross-Origin-Embedder-Policy: require-corp
```

With `require-corp`, future remote images, fonts, scripts, or bundles must use
CORS or a compatible Cross-Origin-Resource-Policy. The demo keeps every Wasm,
Worker, script, and stylesheet on the Pages origin.

Primary references:

- [NAPI-RS WebAssembly and WASI](https://napi.rs/docs/concepts/webassembly)
- [Rust `wasm32-wasip1-threads` target](https://doc.rust-lang.org/rustc/platform-support/wasm32-wasip1-threads.html)
- [wgpu Web/Wasm support](https://github.com/gfx-rs/wgpu)
- [Vello](https://github.com/linebender/vello)
- [COOP](https://developer.mozilla.org/en-US/docs/Web/HTTP/Reference/Headers/Cross-Origin-Opener-Policy) and [COEP](https://developer.mozilla.org/en-US/docs/Web/HTTP/Reference/Headers/Cross-Origin-Embedder-Policy)

Synchronous GPU readback is intentionally not exported: browser WebGPU map
completion is Promise-driven, while the native screenshot path blocks on
device polling. Browser capture needs a separate asynchronous API.
