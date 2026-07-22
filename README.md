# lynx-vello

Rust monorepo exploring a native [Lynx](https://lynxjs.org) rendering stack.

## Workspace layout

| Crate | Purpose |
| --- | --- |
| [`crates/bobcat-engine`](crates/bobcat-engine) | Engine-neutral runtime protocol and composition crate. Independent `resource`, ShadowRealm-inspired `script`, and per-instance `view` modules define host injection and isolated `LynxView<R, E>` ownership without depending on a concrete JavaScript engine. |
| [`crates/bobcat-quickjs`](crates/bobcat-quickjs) | Opaque QuickJS-backed `LynxView` integration over `bobcat-engine` and the Bobcat-independent `quickjs-rust-bridge`. Its public surface is limited to the opaque view, its default constructor and initialization error, plus resource/widget host access; runtime configuration, default constants, explicit-config construction, script adapters, realm/value handles, interrupts, and source evaluation stay internal. |
| [`crates/lynx-template-decoder`](crates/lynx-template-decoder) | Native Rust decoder for the Lynx **web** binary template (`.web.bundle`), a port of `@lynx-js/web-core`'s `decodeTemplate` incl. the rkyv `StyleInfo` model. |
| [`crates/lynx-template-converter`](crates/lynx-template-converter) | Converts source-based native external bundles into web binary templates, translating script routes, native selector/CSS fragments, configuration, and rkyv `StyleInfo`; bytecode/code-cache bundles return a dedicated error because their JavaScript source is not recoverable. |
| [`crates/w3c-dom`](crates/w3c-dom) | Generic W3C-DOM-subset `Document<T>`/`Node<T>` tree and standards-oriented stylo cascade/invalidation core. |
| [`crates/lynx-widget`](crates/lynx-widget) | Lynx Widget/PAPI tree and Lynx-specific style/device adapter over `w3c-dom`. |
| [`crates/neutron-star`](crates/neutron-star) | Statically-dispatched box-layout engine speaking the stylo fork's computed-value vocabulary: CSS Flexbox, numeric CSS Grid Level 2, Starlight `display: linear` and `display: relative`, and shared leaf/cache/positioned/rounding machinery are implemented. |
| [`crates/quickjs-rust-bridge`](crates/quickjs-rust-bridge) | Owner-thread-bound Rust wrapper around the pinned QuickJS C submodule, including exact values, sanitized exceptions, and pending jobs; it is independent of Bobcat and runtime policy. |

`neutron-star` exposes Flex, Grid, Linear, and Relative as peer generic
algorithms over host-owned topology, styles, layout state, and caches. The live
bridge from `lynx-widget` computed styles to retained layout state remains
future work; that integration still needs display dispatch, dirty/cache wiring,
the root fixed-position pass, and text measurement. Its final module or crate
placement has not been established.

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
