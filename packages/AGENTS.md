# Other pnpm packages

Part of the agent guide; the root [AGENTS.md](../AGENTS.md) holds the
mission, the standards and dependency policies, the workspace map, the
toolchain and testing. Read this file before reading or changing code under
`packages/` (other than `packages/bobcat-element`) or
`examples/`. Paths in backticks are relative to the repository root.

- `packages/reactlynx-test-fixtures` — the JSX/CSS/JS sources of the compiled
  ReactLynx cards the integration tests, decoder tests and benchmarks run.
  `rsbuild.config.js` declares the entries and their independent output
  directories, and the package scripts invoke the public `rsbuild build` CLI
  once in production and once in development mode; no script creates a compiler
  or imports an internal build-tool entry point. Run
  `pnpm --filter reactlynx-test-fixtures build` before Rust tests, clippy or
  benches: the emitted `dist/index.rs` registry is what names the bundles, and
  no compiled fixture is versioned. Upstream provenance for the five `basic-*`
  cards and the `react-frame*` cards (lynx-stack's web-core-e2e
  `api-frame-src`/`api-frame-inner`: a red host page whose 100px
  `<frame src="./react-frame-inner.web.bundle">` shows a blue card;
  `api-frame-data` and `api-frame-global-props` as `react-frame-data`;
  `api-frame-auto-height` as `react-frame-auto-height`;
  `api-frame-data-update` as `react-frame-data-update`, whose button's tap
  re-renders the frame's card through that card's own BTS, beside
  `react-frame-global-props-update`, the same for `global-props` (no upstream
  case); `react-frame-tap` and its card `react-frame-tap-inner` (no upstream
  case), a tap inside the frame that runs the card's `catchtap` and
  `main-thread:bindtap` and the host's `bindtap` on the frame's wrapper; all
  rendered end to end by `crates/bobcat-source/tests/frame.rs`) is in that
  package's `NOTICE.lynx-stack`. Native builds use the shared
  `scripts/lynx-bytecode.ts` hook to disable BTS manifest bytecode. MTS keeps
  the compiler's default encoding. The fixture-only source repack remains
  necessary for Bobcat's source evaluator.
- `packages/web-core-e2e-fixtures` — `lynx-stack`'s web-core end-to-end cards,
  vendored as ReactLynx source and compiled here at `engineVersion` 4.1.0. That
  version is the reason to vendor them at all: ReactLynx picks its lazy-bundle
  implementation at compile time, and upstream's own `dist/` is built without
  one, so its whole `basic-lazy-component-*` family calls `__QueryComponent`
  rather than `lynx.fetchBundle`. `groups.js` restates the 21 upstream rspeedy
  config files as one entry per distinct compiler configuration — re-read them
  when re-syncing `src/`, or a case compiles with the wrong switches and the
  suite blames the engine. Its one consumer is
  `crates/bobcat-source/tests/web_core_e2e.rs`, which reads `dist/` and
  `resources/` at run time; its build (`pnpm --filter web-core-e2e-fixtures
  build`) is separate from the fixtures build every other Rust test depends
  on, and CI runs it in the job that runs tests.
- `packages/explorer-homepage`, `packages/explorer-showcase` and
  `packages/explorer-lib` — the Lynx Explorer home screen and showcase menu in
  ReactLynx, over the navigation, launch-command, history and theme helpers the
  `lib` package shares between them.
- `packages/github-pages` — the rsbuild site published to GitHub Pages, built
  by `pnpm build:github-pages`, which builds `bobcat-wasm` and the Explorer
  homepage first and then this package over both.
- `examples/` — `lynx-stack`'s own examples, adapted so their `workspace:*`
  dependencies name published package versions and they install independently
  of the `lynx-stack` source tree. See `examples/README.md` for the TypeScript
  arrangement each one needs.
