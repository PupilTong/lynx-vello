# lynx-vello

Rust and pnpm monorepo exploring a native [Lynx](https://lynxjs.org) rendering stack.

## Workspace layout

| Component | Purpose |
| --- | --- |
| [`crates/bobcat-core`](crates/bobcat-core) | Native runtime core combining resource/script/view protocols with the QuickJS-backed preloaded ESM graph and main-thread host callbacks. It does not expose a renderer façade or re-export DOM/GPU internals. |
| [`crates/bobcat-resources`](crates/bobcat-resources) | Cross-platform reference `ResourceFetcher`: registered and transported bytes, preprocessing, tiered caches, platform image decoding, and per-view image reports remain owned by each embedder's painter thread. |
| [`crates/bobcat-cli`](crates/bobcat-cli) | The independent `bobcat` product: loads local `file:///` web bundles or Lynx XML source cards through `bobcat-source`, privately composes the runtime with a macOS window or paced headless GPU target, and exposes debugger-style frame/screenshot commands. |
| [`crates/bobcat-server`](crates/bobcat-server) | HTTP screenshot embedder compatible with UI Judge's `/health` and `/screenshot` request surface. A bounded queue feeds one owner thread that creates offscreen Bobcat views and returns fixed-size, white-backed BMP captures. |
| [`crates/bobcat-wasm`](crates/bobcat-wasm) | Pure-Rust `wasm-bindgen` browser embedder. An explicit Worker owns the complete engine, crates.io Vello 0.9/wgpu 29, and a transferred `OffscreenCanvas`; it uses `wasm_thread` to run the DOM/style/layout owner in a nested shared-memory Worker. The URL facade loads JavaScript, CSS, or a complete Lynx XML source card while the UI remains a JavaScript-only asynchronous host boundary. |
| [`crates/bobcat-source`](crates/bobcat-source) | Unified Lynx XML, web-bundle and source-based native external-bundle parsing; shared source registration for embedders. [Architecture and API](docs/source-architecture.md). |
| [`crates/dom`](crates/dom) | Generic W3C-DOM-subset `Document<T>`/`Node<T>` tree, standards-oriented Stylo cascade/layout core, and document-owned private paint pipeline. |
| [`packages/bobcat-element`](packages/bobcat-element) | The colocated `bobcat:runtime` compatibility ESM and `bobcat:element` PAPI ESM embedded into `bobcat-core` with `include_str!`. The latter's named `__*` exports own the Element PAPI, Lynx tag vocabulary, native `NodeId` handles, and `WeakRef`/`FinalizationRegistry` lifecycle. |
| [`crates/hughie`](crates/hughie) | Statically-dispatched box-layout engine speaking the stylo fork's computed-value vocabulary: CSS Flexbox, numeric CSS Grid Level 2, Starlight `display: linear` and `display: relative`, and shared leaf/cache/positioned/rounding machinery are implemented. |
| [`crates/quickjs-rust-bridge`](crates/quickjs-rust-bridge) | Owner-thread-bound Rust wrapper around the pinned QuickJS C submodule, including exact values, sanitized exceptions, pending jobs, synchronous source/native-module loading, module namespaces, and Rust-closure-backed host functions; it is independent of Bobcat and runtime policy. |
| [`crates/flashbulb`](crates/flashbulb) | Screenshot testing infrastructure: RGBA images, a `pixelmatch` port matching Playwright's tolerances, and golden-file management. This is to lynx-vello's render tests what Playwright is to lynx-stack's `web-core-e2e` and `web-elements`. |

`hughie` exposes Flex, Grid, Linear, and Relative as peer generic
algorithms over host-owned topology, styles, layout state, and caches.
`dom` is the concrete Stylo-backed host, including display dispatch,
dirty/cache wiring, the positioned pass, text measurement, visual ordering,
and private scene construction. `bobcat-core`'s `tree` module is the native
element layer directly over `dom`, exposed to the Element PAPI as named
functions in the native `bobcat-internal:host` ESM. QuickJS preloads
`bobcat:runtime`, `bobcat:element`, and the resolved entry MTS URL. The
`bobcat:boot` ESM uses top-level await to import that URL,
then calls `processData`, invokes a present `globalThis.renderPage` or
dispatches `__RenderPage` on `lynx.getEngine()`, and finally calls
`__FlushElementTree` inside JavaScript before reporting startup completion.

See [`docs/runtime-architecture.md`](docs/runtime-architecture.md) for the
dependency graph, feature boundary, private paint pipeline, and frame walkthrough.

The repository root is also a pnpm workspace. JavaScript and TypeScript
libraries belong under `packages/*`; runnable integrations and fixtures live
under [`examples/*`](examples/README.md), following the top-level organization
used by `lynx-stack`.

## Running Bobcat

The headed runner currently opens a native window on macOS:

```sh
cargo run -p bobcat-cli --bin bobcat -- \
  -i file:///absolute/path/to/card.web.bundle
```

The same entry point content-sniffs and runs a raw Lynx XML card:

```sh
cargo run -p bobcat-cli --bin bobcat -- \
  -i file:///absolute/path/to/card.xml
```

Headless mode has a configurable synthetic vsync clock:

```sh
cargo run -p bobcat-cli --bin bobcat -- \
  -i file:///absolute/path/to/card.web.bundle \
  --headless --vsync 120
```

Headed and headless sessions accept commands at the `(bobcat)` prompt:
`continue`, `pause`, `frame`, `screenshot [PATH]`, `set vsync FPS` (headless),
`show vsync`, `help`, and `quit`. Screenshots are captured interactively from
the live renderer; there is no one-shot startup flag. The scene builder, GPU
renderer, and render/readback allocations are reused across frames; pixel
readback occurs only for screenshots.

The executable deliberately preserves the runtime's current compatibility
boundary: an input that reaches an unimplemented main-thread global or Element
PAPI member exits with that precise runtime error. Bundle `StyleInfo` is
lowered through the pre-parsed stylesheet contract, while a Lynx XML `<style>`
body uses the raw CSS-text arm. The background-thread runtime is not implemented
yet; an XML background section is retained but reported as not executed.

## Running the screenshot server

`bobcat-server` implements the UI Judge screenshot request shape over Bobcat's
offscreen embedder:

```sh
LYNX_USE_PORT=8080 cargo run -p bobcat-server
curl --fail-with-body \
  --output screenshot.bmp \
  --header 'content-type: application/json' \
  --data '{"url":"file:///absolute/path/to/card.web.bundle","task":"capture"}' \
  http://127.0.0.1:8080/screenshot
```

`GET /health` returns readiness JSON. `POST /screenshot` accepts `file://`,
`http://`, and `https://` web bundles or raw Lynx XML and returns a raw
800×600, DPR-1 BMP after compositing alpha over white. Non-empty
`globalProps`, `initialData`, or interaction `steps` are rejected explicitly
because the current core exposes no faithful injection or automation seam.
Native `.lynx.bundle` bytes are recognized and return `422`; native-template
support belongs to its separate change.

Like UI Judge, the server listens on all IPv4 and IPv6 interfaces and permits
arbitrary file and HTTP(S) input URLs. It has no authentication or TLS, so run
it only in a trusted environment with access to files and networks that callers
are allowed to read, and only with trusted page code. `timeoutMs` bounds the
asynchronous waits it covers, but cannot preempt synchronous JavaScript, GPU
driver work, or teardown already running inside the engine.

## Running the browser embedder

The `github-pages` pnpm package builds the shared-memory `bobcat-wasm` module,
installs a COOP/COEP service worker, and loads a small Lynx XML card through a
complete Worker-owned embedder into its `OffscreenCanvas`. Its query-routed
`?tab=lynx-xml` workspace exposes the demo source in an editor and renders each
submission into the adjacent preview. That embedder starts the DOM owner as a
nested `wasm_thread` Worker and synchronizes the two Rust sides through shared
memory:

```sh
pnpm install --frozen-lockfile
pnpm --filter bobcat-wasm build
pnpm --filter github-pages dev
```

The first visit reloads once after the service worker takes control; this is
required before `SharedArrayBuffer` and threaded Wasm are available. See
[`docs/browser-wasm.md`](docs/browser-wasm.md) for the thread/Canvas design and
the current browser-runtime boundary.

## Toolchain

The workspace pins the **2026-04-20 nightly** toolchain via [`rust-toolchain.toml`](rust-toolchain.toml)
(edition 2024, resolver 3, workspace lints, nightly `rustfmt` options).
Initialize the pinned Stylo and QuickJS sources before the first build:

```sh
git submodule update --init --recursive
cargo check          # uses the pinned nightly automatically
cargo test
cargo fmt
cargo clippy
cargo bench          # divan benchmarks (CodSpeed-compatible)
```

The pnpm workspace follows `lynx-stack`'s toolchain range: Node.js 24 is
recommended (Node.js 22 is also supported), and the exact pnpm release is
pinned through Corepack in [`package.json`](package.json). Initialize it with:

```sh
corepack pnpm install --frozen-lockfile
```

## CI

CI separates native checks, browser Wasm checks, wall-time benchmarks, and
memory benchmarks. Native rustfmt, clippy (`-D warnings`), tests, and coverage
([Codecov](https://codecov.io)) run on `macos-latest` aarch64; the browser job
builds the threaded `wasm-bindgen` artifact and GitHub Pages demo on
Ubuntu. [CodSpeed](https://codspeed.io) tracks wall time on macOS and memory on
Ubuntu.

## Reference knowledge

Deep-dive notes on the Lynx binary template format (encode/decode, "lynx" vs "web"
targets) live in [`docs/`](docs/) and are indexed for agents in
[`.claude/skills/`](.claude/skills/). Source material: the
[`lynx`](https://github.com/lynx-family/lynx) engine repo and the
[`lynx-stack`](https://github.com/lynx-family/lynx-stack) frontend stack repo.
