// The compile-time configuration each case family is built with.
//
// Upstream carries this as 21 rspeedy config files under
// `packages/web-platform/web-core-e2e/tests/reactlynx/`: six that glob a
// family, and fifteen that sit next to a single case. This repository builds
// ReactLynx with Rsbuild directly, so the same information lives here as data
// — one group per distinct compiler configuration, matched by case name.
//
// Keep this table in step with upstream when re-syncing `src/`: a case whose
// family moved would otherwise be compiled with the wrong switches and fail
// the census for a reason that is not the engine's.

/** Cases compiled together, in upstream's own grouping. */
export const groups = [
  // `basic.config.ts` and `api.config.ts`: the plain compiler defaults, which
  // is what the overwhelming majority of the corpus wants.
  {
    name: "default",
    matches: (name) =>
      (name.startsWith("basic-") || name.startsWith("api-"))
      && !name.startsWith("basic-lazy-component-css-selector-false"),
  },
  // `config-css-selector-false.config.ts` and
  // `basic-lazy-component-css-selector-false.config.ts`.
  {
    name: "css-selector-false",
    options: { enableCSSSelector: false },
    matches: (name) =>
      name.startsWith("config-css-selector-false-")
      || name.startsWith("basic-lazy-component-css-selector-false-"),
  },
  // `config-lazy-component.config.ts`. These are lazy-bundle containers, not
  // cards: they carry no `lepusCode.root` and no page opens them — upstream's
  // spec never navigates to one. They go to their own directory so that
  // enumerating `dist/*.web.bundle` yields pages and only pages.
  {
    name: "lazy-component",
    options: { experimental_isLazyBundle: true },
    matches: (name) => name.startsWith("config-lazy-component-"),
    directory: "containers",
  },
  {
    name: "remove-css-scope-false",
    options: { enableRemoveCSSScope: false },
    matches: (name) => name.startsWith("config-css-remove-scope-false"),
  },
  {
    name: "remove-css-scope-true",
    options: { enableRemoveCSSScope: true },
    matches: (name) => name === "config-css-remove-scope-true",
  },
  {
    name: "css-inheritance-true",
    options: { enableCSSInheritance: true },
    matches: (name) => name === "config-css-inheritance-true",
  },
  // Two more cards over the same source, to be read against the one above.
  // Upstream gives each its own config file; the entry is what differs.
  {
    name: "css-inheritance-false",
    options: { enableCSSInheritance: false },
    entries: { "config-css-inheritance-false": "config-css-inheritance-true" },
  },
  {
    name: "css-inheritance-default",
    entries: { "config-css-inheritance-default": "config-css-inheritance-true" },
  },
  {
    name: "default-display-linear-false",
    options: { defaultDisplayLinear: false },
    matches: (name) => name === "config-css-default-display-linear-false",
  },
  // Upstream passes no switch here: the case is about the page default the
  // engine applies when the card says nothing.
  {
    name: "default-overflow-visible-unset",
    matches: (name) => name === "config-css-default-overflow-visible-unset",
  },
  {
    name: "mixed-01",
    options: { enableRemoveCSSScope: false, enableCSSSelector: false },
    matches: (name) => name === "config-mixed-01",
  },
  // The three split-chunk cases each own a directory under `dist/`, because
  // their vendor chunks are separate files the card fetches by relative path.
  {
    name: "splitchunk-single-vendor",
    options: { firstScreenSyncTiming: "jsReady" },
    matches: (name) => name === "config-splitchunk-single-vendor",
    ownDirectory: true,
    splitChunks: { preset: "single-vendor", filename: "[name].[contenthash:8].js" },
  },
  {
    name: "splitchunk-split-by-experience",
    options: { firstScreenSyncTiming: "jsReady" },
    matches: (name) => name === "config-splitchunk-split-by-experience",
    ownDirectory: true,
    // Upstream names rspeedy's `split-by-experience`; Rsbuild 2 calls that
    // same strategy `default` (`chunkSplit.strategy || 'split-by-experience'`).
    splitChunks: { preset: "default", filename: "[name].[contenthash:8].js" },
  },
  {
    name: "splitchunk-split-by-module",
    options: { firstScreenSyncTiming: "jsReady" },
    matches: (name) => name === "config-splitchunk-split-by-module",
    ownDirectory: true,
    // Rsbuild 2's nearest name for rspeedy's `split-by-module`: a chunk per
    // package rather than per module. The card is about loading more than one
    // chunk, which either strategy gives it.
    splitChunks: { preset: "per-package", filename: "[name].[contenthash:8].js" },
  },
  // A development-mode card whose asset prefix deliberately points nowhere:
  // the case is that a card still comes up when its own assets 404.
  {
    name: "mode-dev-with-all-in-one",
    matches: (name) => name === "config-mode-dev-with-all-in-one",
    ownDirectory: true,
    mode: "development",
    assetPrefix: "error://example.com/",
  },
];

/** The group a case belongs to, or undefined when nothing claims it. */
export function groupOf(caseName) {
  return groups.find((group) => group.matches?.(caseName));
}
