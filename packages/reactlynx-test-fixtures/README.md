# ReactLynx test fixtures

This private pnpm workspace owns the JSX/CSS/JS sources used by Bobcat's
compiled ReactLynx integration tests, decoder tests and benchmarks. The five
`basic-*` cards come from lynx-stack; see `NOTICE.lynx-stack` for provenance.

## Build

```sh
pnpm install --frozen-lockfile
pnpm --filter reactlynx-test-fixtures build
```

`rsbuild.config.js` declares the entries, independent output directories and
`pluginReactLynx({ engineVersion: '4.1.0' })`. Other ReactLynx options keep their
defaults. The package scripts invoke the public `rsbuild build` CLI once in
production mode and once in development mode. No script creates a compiler or
imports an internal build-tool entry point.
Both commands set `NODE_ENV` explicitly so the fixture matrix and compiler
use the same mode.

The default build produces eight native production pages, three native development
variants and six web pages. Each native environment has its own
compilation, so its lazy chunks cannot be shared with another test page.

| Environment | Fixture behavior |
| --- | --- |
| `lynx-react-lazy` | Lazy/Suspense, a BTS effect and child CSS |
| `lynx-react-lazy-sync` | First-screen sync import and event-triggered import |
| `lynx-react-lazy-nested` | Nested async/sync imports, shared BTS module and CSS |
| `lynx-react-reload` | State, effect cleanup, host/BTS reload and entry counter |
| `lynx-react-data-processor` | Synchronous default/named processors and data updates |
| `lynx-react-global-props` | Reactive global props, initial state and clicks |
| `lynx-react-list` | A forty-cell `<list>`, and a tap that removes three cells and appends one |
| `lynx-react-native`, `web-react-bts-query` | Ref fields, typed dataset, scoped query and native props |
| `web-basic-bindtap` | Event delivery and state updates |
| `web-basic-class-selector`, `web-basic-performance-large-css` | CSS decoding and rendered cards |
| `web-basic-mts-run-on-main-thread`, `web-basic-mts-run-on-background` | Worklets and main-thread refs |

Development builds select `lynx-react-reload`, `lynx-react-global-props` and
`lynx-react-lazy-nested`, retaining the compiler's default HMR client and asset
prefix. Select a single environment through the CLI:

```sh
pnpm --filter reactlynx-test-fixtures build:production --environment lynx-react-lazy
pnpm --filter reactlynx-test-fixtures build:development --environment lynx-react-reload
pnpm --filter reactlynx-test-fixtures build:production --environment web-react-bts-query
```

`pnpm test:type` includes the configuration and TypeScript build hook. ReactLynx
compiles the fixture inputs. Processors return their data synchronously; the
fixture does not require nested Promise jobs to run before that return value
is consumed.

## Outputs

Outputs live only in ignored `dist/`; compiled bundles and provenance must not
be committed. Native pages retain `dist/<fixture>/<fixture>.lynx.bundle` and
`dist/<fixture>/async/*`. Development directory names append `-development`.
Web pages are `dist/<fixture>/<fixture>.web-<fixture>.bundle`.

The shared `scripts/lynx-bytecode.ts` hook disables BTS manifest bytecode without
changing the native page layout. The existing fixture-only adapter in
`scripts/source-bundles.ts` registers Rsbuild completion hooks. For native
pages it reads the compiler input retained by `DEBUG=lynx` and repacks the
original MTS/BTS source into external custom sections using `@lynx-js/tasm`.
It replaces the page's bytecode container, preserves emitted lazy bundle bytes
and leaves web output unchanged. `DEBUG=lynx` also retains MTS source inside
lazy bundles; these fixtures exercise source evaluation, not native bytecode.

The encoder is pinned to `@lynx-js/tasm@0.0.53`. A workspace override keeps the
compiler and repacker on that same version; the previous encoder cannot target
engine 4.1.0. The hook still relies on the pinned compiler's debug `tasm.json`
format, which should be checked when upgrading the toolchain.

Each environment writes `dist/<output-directory>.provenance.json` with its actual
source dependencies, compiler versions and bundle hashes. Native records also
include source-section hashes, the original page hash and its public path.
Chunk names may change with paths, compiler versions and development hashes;
consumers should use the provenance map instead of guessing filenames.

Historical 3.5/native-example and QueryComponent generators are removed. This
workspace targets the selected 4.1.0/default-options fixtures only.

The final build hook writes `dist/index.rs` from all provenance records.
`fixtures.rs` includes that generated registry for native/Wasm tests and
benchmarks, so consumers use the compiler's actual chunk names and public paths.
Run the fixture build before `cargo test`, `cargo clippy --all-targets` or
benchmark compilation; CI does the same. Generated JS, bundles and the registry
stay out of version control.

Lazy fixtures remain available as source inputs for later work. Their external
bundle loading is not part of the current runtime integration suite.
