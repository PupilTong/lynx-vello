# lynx-vello

Rust and pnpm monorepo exploring a native [Lynx](https://lynxjs.org) rendering stack.

## Workspace layout

| Crate | Purpose |
| --- | --- |
| [`crates/bobcat-core`](crates/bobcat-core) | Native runtime core combining engine-neutral resource/script/view protocols with the optional QuickJS adapter and main-thread host globals. It does not expose a renderer façade or re-export DOM/GPU internals. |
| [`crates/bobcat-cli`](crates/bobcat-cli) | The independent `bobcat` product: loads local `file:///` web bundles, privately composes the runtime with a macOS window or paced headless GPU target, and exposes debugger-style frame/screenshot commands. |
| [`crates/lynx-template-decoder`](crates/lynx-template-decoder) | Native Rust decoder for the Lynx **web** binary template (`.web.bundle`), a port of `@lynx-js/web-core`'s `decodeTemplate` incl. the rkyv `StyleInfo` model. |
| [`crates/dom`](crates/dom) | Generic W3C-DOM-subset `Document<T>`/`Node<T>` tree, standards-oriented Stylo cascade/layout core, and document-owned private paint pipeline. |
| [`crates/lynx-element`](crates/lynx-element) | The Lynx runtime element layer: `ElementId = u32`, validated Element PAPI operations and handle space, `ElementTree`, `<page>` root policy, view/device construction, and Lynx UA defaults. |
| [`crates/hughie`](crates/hughie) | Statically-dispatched box-layout engine speaking the stylo fork's computed-value vocabulary: CSS Flexbox, numeric CSS Grid Level 2, Starlight `display: linear` and `display: relative`, and shared leaf/cache/positioned/rounding machinery are implemented. |
| [`crates/pulsar`](crates/pulsar) | DOM-independent Vello resources and GPU submission: `ImageStore`, Vello re-exports, and the retained headless render-to-texture backend. |
| [`crates/quickjs-rust-bridge`](crates/quickjs-rust-bridge) | Owner-thread-bound Rust wrapper around the pinned QuickJS C submodule, including exact values, sanitized exceptions, pending jobs, and Rust-closure-backed host functions; it is independent of Bobcat and runtime policy. |
| [`crates/flashbulb`](crates/flashbulb) | Screenshot testing infrastructure: RGBA images, a `pixelmatch` port matching Playwright's tolerances, and golden-file management. This is to lynx-vello's render tests what Playwright is to lynx-stack's `web-core-e2e` and `web-elements`. |

`hughie` exposes Flex, Grid, Linear, and Relative as peer generic
algorithms over host-owned topology, styles, layout state, and caches.
`dom` is the concrete Stylo-backed host, including display dispatch,
dirty/cache wiring, the positioned pass, text measurement, visual ordering,
and private scene construction. `lynx-element` is the runtime adapter directly
over `dom`; `bobcat-core` composes it with runtime protocols, and the core's
optional QuickJS feature runs main-thread scripts
against it — five of web-core's 61 Element PAPI members are wired up so far
(`__CreatePage`, `__CreateView`,
`__AppendElement`, `__DropElement`, `__FlushElementTree`);
`StyleInfo` ingestion, attributes, classes, and events are not.

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
boundary: a real bundle that reaches an unimplemented main-thread global or
Element PAPI member exits with that precise runtime error. Decoded `StyleInfo`
author rules are not ingested yet; if a runnable bundle contains them, the CLI
prints an explicit warning that author styles are omitted.

## Toolchain

The workspace pins the **2026-07-01 nightly** toolchain via [`rust-toolchain.toml`](rust-toolchain.toml)
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

A single `macos-latest` (aarch64) job runs rustfmt, clippy (`-D warnings`),
tests with coverage ([Codecov](https://codecov.io)), and benchmarks tracked by
[CodSpeed](https://codspeed.io) in **walltime** mode — CodSpeed's
valgrind-based simulation instrument is Linux-only, but walltime is fully
supported on macOS aarch64 (runner ships darwin-arm64 binaries and uses the
samply profiler there).

## Reference knowledge

Deep-dive notes on the Lynx binary template format (encode/decode, "lynx" vs "web"
targets) live in [`docs/`](docs/) and are indexed for agents in
[`.claude/skills/`](.claude/skills/). Source material: the
[`lynx`](https://github.com/lynx-family/lynx) engine repo and the
[`lynx-stack`](https://github.com/lynx-family/lynx-stack) frontend stack repo.
