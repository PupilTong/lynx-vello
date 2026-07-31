# Lynx examples

These examples are adapted from `lynx-stack` commit
[`216b1b3adbd3b139a32f953f9d40b87c806f0b26`](https://github.com/lynx-family/lynx-stack/commit/216b1b3adbd3b139a32f953f9d40b87c806f0b26).
Their `workspace:*` dependencies have been replaced with the corresponding
published package versions so they can be installed independently of the
`lynx-stack` source tree. The original Apache 2.0 license and notice are
preserved in this directory.

The published peer metadata for `@lynx-js/rspeedy@0.16.0` accepts TypeScript
through 5.9, while `@lynx-js/react@0.123.0` accepts React 18 type definitions.
Accordingly, this workspace pins TypeScript 5.9.3 and `@types/react` 18.3.28
until matching packages with the newer peer ranges are published.

From the repository root:

```sh
pnpm build:examples
pnpm test:examples:type
pnpm test:examples
```

To work with a single example:

```sh
pnpm --filter @lynx-js/example-react dev
```
