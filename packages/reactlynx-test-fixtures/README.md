# ReactLynx test fixtures

This private pnpm workspace owns 13 JSX/CSS/JS fixture sources extracted unchanged
from BTS checkpoint `e04e4168`. It builds independently of Bobcat and its Rust
submodules. Runtime integration tests are a separate change.

## Build

```sh
pnpm install --frozen-lockfile
pnpm --filter reactlynx-test-fixtures build
```

`lynx.config.js` declares the entries, independent output directories and
`pluginReactLynx({ engineVersion: '4.1.0' })`. Other ReactLynx options keep their
defaults. The package scripts invoke the public `rspeedy build` CLI once in
production mode and once in development mode. No script creates a compiler or
imports an internal Rspeedy entry point.
Both commands set `NODE_ENV` explicitly: Rspeedy reads the config function's
`env` before applying `--mode`, and the fixture matrix must match that mode.

The default build produces six native production pages, three native development
variants and the web node-query page. Each native environment has its own
compilation, so its lazy chunks cannot be shared with another test page.

| Environment | Fixture behavior |
| --- | --- |
| `lynx-react-lazy` | Lazy/Suspense, a BTS effect and child CSS |
| `lynx-react-lazy-sync` | First-screen sync import and event-triggered import |
| `lynx-react-lazy-nested` | Nested async/sync imports, shared BTS module and CSS |
| `lynx-react-reload` | State, effect cleanup, host/BTS reload and entry counter |
| `lynx-react-data-processor` | Default/named processors, Promise jobs and data updates |
| `lynx-react-global-props` | Reactive global props, initial state and clicks |
| `web` | Ref fields, typed dataset, scoped query and native props |

Development builds select `lynx-react-reload`, `lynx-react-global-props` and
`lynx-react-lazy-nested`, retaining the compiler's default HMR client and asset
prefix. Select a single environment through the CLI:

```sh
pnpm --filter reactlynx-test-fixtures build:production --environment lynx-react-lazy
pnpm --filter reactlynx-test-fixtures build:development --environment lynx-react-reload
pnpm --filter reactlynx-test-fixtures build:production --environment web
```

`pnpm test:type` includes the configuration and TypeScript build hook. The JSX
fixture inputs keep their original bytes and are compiled by ReactLynx.

## Outputs

Outputs live only in ignored `dist/`; compiled bundles and provenance must not
be committed. Native pages retain `dist/<fixture>/<fixture>.lynx.bundle` and
`dist/<fixture>/async/*`. Development directory names append `-development`.
The web page is `dist/react-bts-query/react-bts-query.web.bundle`.

`scripts/source-bundles.ts` registers one Rsbuild completion hook. For native
pages it reads the compiler input retained by `DEBUG=rspeedy` and repacks the
original MTS/BTS source into external custom sections using `@lynx-js/tasm`.
It replaces the page's bytecode container, preserves emitted lazy bundle bytes
and leaves web output unchanged. `DEBUG=rspeedy` also retains MTS source inside
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
