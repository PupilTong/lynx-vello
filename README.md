# lynx-vello

Rust monorepo exploring a native [Lynx](https://lynxjs.org) rendering stack.

## Workspace layout

| Crate | Purpose |
| --- | --- |
| [`crates/lynx-template-decoder`](crates/lynx-template-decoder) | Native Rust decoder for the Lynx **web** binary template (`.web.bundle`), a port of `@lynx-js/web-core`'s `decodeTemplate` incl. the rkyv `StyleInfo` model. |

## Toolchain

The workspace pins the latest **nightly** toolchain via [`rust-toolchain.toml`](rust-toolchain.toml)
(edition 2024, resolver 3, workspace lints, nightly `rustfmt` options).

```sh
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
