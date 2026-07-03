# lynx-vello

Rust monorepo exploring a native Lynx rendering stack. Cargo workspace pinned to
the latest **nightly** toolchain (`rust-toolchain.toml`), edition 2024, resolver 3,
workspace lints. Format with `cargo fmt` (nightly rustfmt options in `rustfmt.toml`).

## Crates

- `crates/lynx-template-decoder` — native Rust decoder for the Lynx **web** binary
  template (`.web.bundle`, magic `SDRA WROF`). Scope: binary template parsing only;
  no JS runtime, no CSS engine.

## Reference knowledge (read before touching template/format code)

- [docs/web-binary-template.md](docs/web-binary-template.md) — the web target
  bundle format this repo decodes: container layout, section encodings, and the
  rkyv 0.7 `RawStyleInfo` CSS data model (mirrored 1:1 in the decoder crate —
  field/variant order there is wire format, do not reorder).
- [docs/lynx-binary-template.md](docs/lynx-binary-template.md) — the *native*
  `.lynx.bundle` format ("lynx" target): header, section route, Lepus value
  encoding. Not implemented here yet; kept for reference.

Source repos consulted (local checkouts):
- `/Users/akiwah/repos/lynx` — Lynx engine (C++ encoder/decoder of `.lynx.bundle`).
- `/Users/akiwah/repos/lynx-stack` — rspeedy build stack + web platform runtime
  (TS + Rust/wasm reference implementation of the web bundle codec in
  `packages/web-platform/web-core`).

## Testing

Integration tests decode real fixtures vendored from lynx-stack under
`crates/lynx-template-decoder/tests/fixtures/` (Apache-2.0 build artifacts).
`cargo test` must pass on the pinned nightly.
