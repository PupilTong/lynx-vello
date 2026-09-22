# Lynx examples

These examples are adapted from `lynx-stack` commit
[`216b1b3adbd3b139a32f953f9d40b87c806f0b26`](https://github.com/lynx-family/lynx-stack/commit/216b1b3adbd3b139a32f953f9d40b87c806f0b26).
Their `workspace:*` dependencies have been replaced with the corresponding
published package versions so they can be installed independently of the
`lynx-stack` source tree. The original Apache 2.0 license and notice are
preserved in this directory.

TypeScript 7.0.2 at the workspace root type-checks every example: each
example's `tsconfig.json` extends the workspace `tsconfig.base.json`, and
`pnpm test:type` checks the whole workspace, examples included. Rsbuild loads
`rsbuild.config.ts` directly, so the examples use the workspace TypeScript
version without a separate compiler for the config loader.
`@lynx-js/react@0.126.1` accepts React 18 type definitions, so the examples
keep `@types/react` 18.3.28.

From the repository root:

```sh
pnpm build:examples
pnpm test:type
pnpm test:examples
```

To work with a single example:

```sh
pnpm --filter @lynx-js/example-react dev
pnpm exec tsc -b examples/react
```

## Native source bundles

The Rsbuild configurations use `scripts/lynx-bytecode.ts` to emit native
JavaScript as source custom sections. Pages use the external bundle layout:
`<entry>__main-thread` holds MTS source, `app-service.js` starts BTS, and named
CSS sections preserve styles. Load these pages in Bobcat with the named MTS
entry. FetchBundle chunks keep their `main-thread`, `background` and `CSS`
section names.

Native external libraries set `enableJsBytecode: false` in their Rslib encoder
options. The `react-externals` build also rebuilds React UMD from its published
TypeScript sources in both production and development modes. Its native preset
copies those local bundles instead of the dependency's precompiled bundles.
Run that example's package scripts so its external libraries are built first.

Source output does not require `DEBUG`. Web builds keep their `JsBytecode`
section tags because the web encoder uses them to identify MTS source; it does
not compile that source to bytecode. The native encoder still writes an inert
empty root stub required by its external bundle format. Application code lives
entirely in source sections, and Bobcat's decoder rejects executable bytecode.
