# web-core-e2e-fixtures

`lynx-stack`'s web-core end-to-end cards, vendored as ReactLynx source and
compiled here for the engine version this repository targets.

The cards come from `packages/web-platform/web-core-e2e/tests/reactlynx` and
keep their Apache-2.0 headers; `NOTICE.lynx-stack` and `LICENSE.lynx-stack`
carry the notice, as they do for `packages/reactlynx-test-fixtures`. Playwright's
screenshots of web-core rendering these same cards stay upstream — the census
reads them from a `lynx-stack` checkout — so this package holds sources and a
build, never goldens.

## Why vendor them rather than build them upstream

ReactLynx picks its lazy-bundle implementation at compile time. Only
`engineVersion` 3.9 and above compile against `lynx.fetchBundle`; below that the
card calls `__QueryComponent`, which this engine does not implement and, by the
scope decision, will not. Upstream's own `dist/` is built without an
`engineVersion`, so its whole `basic-lazy-component-*` family exercises a path
this engine has no intention of serving. Compiled here at `4.1.0`, the same
sources exercise the path it does.

Everything else about the corpus is upstream's: the cards, their CSS and their
assets are copied without edits.

## Building

```sh
pnpm --filter web-core-e2e-fixtures build          # every group
node scripts/build.js default                      # one group
E2E_GROUP=default npx rsbuild build                # one group, directly
```

Output is `dist/<case>.web.bundle`, the layout upstream's goldens were taken
against and the one `.github/scripts/web-core-census.py` reads. Two families
sit elsewhere:

- the three `config-splitchunk-*` cards and `config-mode-dev-with-all-in-one`
  write into `dist/<case>/`, because they load further chunks by relative path;
- `config-lazy-component-*` writes into `dist/containers/`. Those are
  lazy-bundle containers rather than cards — they carry no `lepusCode.root`,
  and upstream's spec never opens one — so keeping them out of `dist/`'s top
  level is what makes "every `dist/*.web.bundle`" mean "every page".

This build is not part of `pnpm --filter reactlynx-test-fixtures build`, which
the Rust tests need before every `cargo` run. Hundreds of cards are minutes of
work and nothing in `crates/` reads them; the census is their only consumer.

## `groups.js`

Upstream expresses per-case compiler configuration as 21 rspeedy config files:
six that glob a family, fifteen that sit beside a single case. This repository
builds ReactLynx with Rsbuild directly, so that information lives in
`groups.js` as one entry per distinct configuration, matched by case name —
`enableCSSSelector: false`, `enableRemoveCSSScope: false`,
`experimental_isLazyBundle: true`, the two `enableCSSInheritance` variants, the
split-chunk presets, and the development-mode card whose asset prefix points at
a host that does not resolve.

**When re-syncing `src/` from upstream, re-read those config files.** A case
whose family moved would otherwise compile with the wrong switches, and the
census would blame the engine for it.

## What is not built

`external-bundle` needs `external-libs/greeting`, an external bundle upstream
builds with `@lynx-js/lynx-bundle-rslib-config` and serves from its own
`resources/`. That library is not vendored here, so the card's source is
present and unbuilt. It is the one card in the corpus that exercises
`lynx.fetchBundle` against a foreign container rather than a lazy child.
