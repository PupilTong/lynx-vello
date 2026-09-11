# ReactLynx test fixtures

This private pnpm workspace owns the ReactLynx JSX/CSS/JS test applications and
their bundle generators, extracted from the BTS implementation checkpoint
`e04e4168`. The 13 files in `src/` retain that checkpoint's source bytes.
Rust runtime changes, integration tests and generated bundles are separate
integration steps; this package builds without Bobcat or its Rust submodules.

## Build

From the repository root:

```sh
pnpm install --frozen-lockfile
pnpm --filter reactlynx-test-fixtures build
```

The command builds six production fixtures and three development variants into
this package's ignored `dist/` directory. Each output has a companion
`*.provenance.json` with compiler versions, script hashes and bundle hashes.
The package is included by the existing `packages/*` workspace glob.

Only `engineVersion: '4.1.0'` is selected; other ReactLynx plugin options retain
their defaults. This is the latest stable engine verified for the BTS work on
2026-09-11. `@lynx-js/tasm@0.0.53` supports that target; the workspace override
keeps the compiler and repacker on the same encoder. The old 0.0.39 encoder
rejects targets above 3.9. The fixture package owns the direct dependency.

Build a single fixture, including a development variant:

```sh
pnpm --filter reactlynx-test-fixtures build:fixture react-lazy-nested
pnpm --filter reactlynx-test-fixtures build:fixture react-reload development
```

| Fixture source | Behavior exercised by the application |
| --- | --- |
| `react-lazy` and child JSX/CSS | Lazy/Suspense, a BTS effect and child CSS |
| `react-lazy-sync` | Synchronous first-screen import and a later event-triggered import |
| `react-lazy-nested`, outer/inner JSX, CSS and value JS | Async outer/sync inner imports, shared BTS-only module and state-update colors |
| `react-reload` | State, effect mount/cleanup, host/BTS reload and an entry execution counter |
| `react-data-processor` | Default/named processors, nested Promise work, update/reset/reload and clicks |
| `react-global-props` | Reactive global props, initial state and clicks |
| `react-bts-query` | Ref fields, typed dataset, scoped query and native props |

## Source-only native output

The current generator builds the `lynx` target with `DEBUG=rspeedy` to preserve
the compiler's source for lazy MTS sections. It repacks the page's original
MTS/BTS sources into external custom sections. Lazy bundles retain their emitted
bytes. This is a source-container fixture, not execution of native bytecode.
Generated names can change when paths, compiler versions or HMR hashes change;
consumers should read the provenance bundle map instead of guessing chunk names.
Development keeps the compiler's default HMR client and asset prefix.

## Historical generators

The existing generator code is retained for earlier evidence. These commands
are separate from the latest-engine build and do not expand its compatibility
target:

```sh
pnpm --filter reactlynx-test-fixtures build:legacy-query
pnpm --filter reactlynx-test-fixtures build:legacy-native
```

`build:legacy-query` emits the ref-query web fixture and the old default-engine
QueryComponent lazy fixture. It uses this package's sources and dependencies.
`build:legacy-native` first builds the existing `examples/react` workspace, then
repackages its native BTS and web MTS into the original style-free external
fixture. That historical fixture intentionally uses the example's source; it
does not borrow the example's `node_modules`. Neither command writes into Rust
test directories or overwrites checked-in fixture artifacts.
