# Lynx examples

These examples are adapted from `lynx-stack` commit
[`216b1b3adbd3b139a32f953f9d40b87c806f0b26`](https://github.com/lynx-family/lynx-stack/commit/216b1b3adbd3b139a32f953f9d40b87c806f0b26).
Their `workspace:*` dependencies have been replaced with the corresponding
published package versions so they can be installed independently of the
`lynx-stack` source tree. The original Apache 2.0 license and notice are
preserved in this directory.

TypeScript 7.0.2 at the workspace root type-checks every example: each
example's `tsconfig.json` extends the workspace `tsconfig.base.json`, and
`pnpm test:type` checks the whole workspace, examples included. Each example
also installs TypeScript 5.9.3, used only by the Node module hook through which
`@lynx-js/rspeedy@0.16.0` loads `lynx.config.ts`: the hook imports the classic
TypeScript compiler API, which TypeScript 7 does not provide, and rspeedy's
published peer range stops at 5.9. That local 5.9.3 is not the type checker;
`pnpm --filter <example> exec tsc` would run it instead of 7.0.2.
`@lynx-js/react@0.123.0` accepts React 18 type definitions, so the examples
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
