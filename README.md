# lynx-vello

Rust monorepo exploring a native [Lynx](https://lynxjs.org) rendering stack.

## Workspace layout

| Crate | Purpose |
| --- | --- |
| [`crates/bobcat-engine`](crates/bobcat-engine) | Engine-neutral runtime protocol and composition crate. Independent `resource`, ShadowRealm-inspired `script`, and per-instance `view` modules define host injection and isolated `LynxView<R, E>` ownership without depending on a concrete JavaScript engine or DOM adapter. |
| [`crates/bobcat-quickjs`](crates/bobcat-quickjs) | Opaque QuickJS-backed `LynxView` integration over `bobcat-engine` and the Bobcat-independent `quickjs-rust-bridge`, plus the Lynx host globals: `MainThreadRuntime` installs the Element PAPI and runs a `.web.bundle`'s main-thread (MTS) script. Runtime configuration, default constants, explicit-config construction, script adapters, realm/value handles, interrupts, and raw source evaluation stay internal. |
| [`crates/bobcat-cli`](crates/bobcat-cli) | The `bobcat` executable: loads local `file:///` web bundles through `bobcat-quickjs`, drives one shared layout/paint scene pipeline into either a macOS window or a paced headless GPU target, and exposes debugger-style frame/screenshot commands. |
| [`crates/lynx-template-decoder`](crates/lynx-template-decoder) | Native Rust decoder for the Lynx **web** binary template (`.web.bundle`), a port of `@lynx-js/web-core`'s `decodeTemplate` incl. the rkyv `StyleInfo` model. |
| [`crates/dom`](crates/dom) | Generic W3C-DOM-subset `Document<T>`/`Node<T>` tree and standards-oriented stylo cascade/invalidation core. |
| [`crates/lynx-element`](crates/lynx-element) | The Lynx runtime element layer over `dom`: Element PAPI handles and the unique-id space, `<page>` root policy, view/device construction, and the Lynx UA cascade defaults. |
| [`crates/hughie`](crates/hughie) | Statically-dispatched box-layout engine speaking the stylo fork's computed-value vocabulary: CSS Flexbox, numeric CSS Grid Level 2, Starlight `display: linear` and `display: relative`, and shared leaf/cache/positioned/rounding machinery are implemented. |
| [`crates/pulsar`](crates/pulsar) | The vello-backed paint engine: turns a `dom` paint order into a GPU scene, plus a headless render-to-texture path. |
| [`crates/quickjs-rust-bridge`](crates/quickjs-rust-bridge) | Owner-thread-bound Rust wrapper around the pinned QuickJS C submodule, including exact values, sanitized exceptions, pending jobs, and Rust-closure-backed host functions; it is independent of Bobcat and runtime policy. |
| [`crates/flashbulb`](crates/flashbulb) | Screenshot testing infrastructure: RGBA images, a `pixelmatch` port matching Playwright's tolerances, and golden-file management. This is to lynx-vello's render tests what Playwright is to lynx-stack's `web-core-e2e` and `web-elements`. |

`hughie` exposes Flex, Grid, Linear, and Relative as peer generic
algorithms over host-owned topology, styles, layout state, and caches.
`dom` is the concrete Stylo-backed host, including display dispatch,
dirty/cache wiring, the positioned pass, and text measurement. `lynx-element`
is the runtime adapter above it, and `bobcat-quickjs` runs main-thread scripts
against it — four of web-core's 61 Element PAPI members are wired up so far
(`__CreatePage`, `__CreateView`, `__AppendElement`, `__FlushElementTree`);
`StyleInfo` ingestion, attributes, classes, and events are not.

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
