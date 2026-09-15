# Lynx Explorer pages

Three pnpm packages hold the ReactLynx pages of Lynx Explorer, copied from
`explorer/` in [lynx-family/lynx](https://github.com/lynx-family/lynx) at
commit
[`84cbc99d82281a132bbbcdceca43192082036700`](https://github.com/lynx-family/lynx/tree/84cbc99d82281a132bbbcdceca43192082036700/explorer):

| Package | Directory | Upstream |
| --- | --- | --- |
| `@explorer/lib` | `packages/explorer-lib` | `explorer/lib` |
| `@explorer/homepage` | `packages/explorer-homepage` | `explorer/homepage` |
| `@explorer/showcase` | `packages/explorer-showcase` | `explorer/showcase/menu` and the `@lynx-example/*` dependencies of `explorer/showcase` |

The original Apache 2.0 license and notice are kept in each package as
`LICENSE.lynx` and `NOTICE.lynx`.

## Differences from upstream

- The Lynx build scripts (`build.py`, `build_and_copy.py`) are not copied. The
  latter copied the showcase bundles into the native app resources; the
  `@lynx-example/*` bundles are available under
  `packages/explorer-showcase/node_modules/@lynx-example/*/dist` instead.
- The showcase wrapper package and its `menu` package are merged into one.
- Sparkling is not copied. Every route opens through
  `NativeModules.ExplorerModule.openSchema`; the Sparkling container and
  runtime switch, the Sparkling Go extension, the `hybrid://` launch dialect,
  `sparkling-navigation` and the Sparkling UI and styles are removed. The
  launch history no longer records a runtime.
- The upstream vitest tests are not copied.
- Dependencies use the workspace catalog, so the pages build with the latest
  `@lynx-js/*` packages instead of the versions pinned upstream. Unused
  dependencies (`react-dom`, `react-router-dom`, `zustand`, `sparkling-method`,
  `prettier`) are dropped.
- The `@lynx-example/*` packages come from the `lynx-examples` catalog in
  `pnpm-workspace.yaml`: pkg.pr.new builds of
  [lynx-family/lynx-examples#440](https://github.com/lynx-family/lynx-examples/pull/440),
  pinned to its head commit.
- Both pages build a `web` environment next to the upstream `lynx` one.
- The sources type-check under the workspace's strict `tsconfig.base.json`.
  The changes for that are type-level (index-signature access, optional
  properties, type-only imports), with two exceptions: `MenuItem` reads
  `props` instead of the component's `this.props`, and the host globals
  `@explorer/lib` reads are declared once in its `typing.d.ts`.

## Commands

From the repository root:

```sh
pnpm build:explorer
pnpm test:type
```
