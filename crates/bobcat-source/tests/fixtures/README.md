# Test fixtures

All compiled ReactLynx inputs are built from the
[`reactlynx-test-fixtures` pnpm workspace](../../../../packages/reactlynx-test-fixtures/README.md).
Run `pnpm --filter reactlynx-test-fixtures build` before Rust tests or benchmarks.
The package's `fixtures.rs` registry includes outputs from ignored `dist/`.
No compiled bundle is kept here.

The `basic-class-selector`, `basic-bindtap` and `basic-performance-large-css`
source cards retain the CSS, empty-StyleInfo and large-StyleInfo decoder cases.
