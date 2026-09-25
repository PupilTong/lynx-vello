import fs from 'node:fs';
import path from 'node:path';

import { pluginReactLynx } from '@lynx-js/react-rsbuild-plugin';
import { defineConfig } from '@rsbuild/core';

import { groups, groupOf } from './groups.js';

// The engine version every card here is compiled for. It is the one difference
// from upstream's own build, and it is the point of vendoring the corpus:
// ReactLynx picks its lazy-bundle implementation at compile time, and only
// 3.9 and above compile against `lynx.fetchBundle` rather than the legacy
// `__QueryComponent` path this engine does not implement.
const ENGINE_VERSION = '4.1.0';
// Cards resolve their own assets against this at runtime, so it has to match
// the static server the census runs (`.github/scripts/web-core-census.py`).
const ASSET_PREFIX = `http://localhost:${process.env['PORT'] ?? 3080}/dist`;
// Built from `external-libs/greeting` upstream, which is not vendored here, so
// the card that fetches it cannot be built yet. Its source is kept all the
// same: the gap is the library, not the case.
const UNBUILDABLE = new Set(['external-bundle']);

// `src/cases` keeps upstream's own depth: a handful of cards import assets
// as `../../../resources/…`, which from `<package>/src/cases/<case>` is
// `<package>/resources`, the directory those four files were copied into.
const SOURCE = path.join(import.meta.dirname, 'src', 'cases');

/** Every case directory, in the order the census will meet them. */
function cases() {
  return fs.readdirSync(SOURCE, { withFileTypes: true })
    .filter((entry) => entry.isDirectory())
    .map((entry) => entry.name)
    .filter((name) => !UNBUILDABLE.has(name))
    .sort();
}

function entriesOf(group) {
  if (group.entries) {
    return Object.fromEntries(Object.entries(group.entries).map(([name, directory]) => [
      name,
      path.join(SOURCE, directory, 'index.jsx'),
    ]));
  }
  return Object.fromEntries(cases()
    .filter((name) => groupOf(name) === group)
    .map((name) => [name, path.join(SOURCE, name, 'index.jsx')]));
}

const selected = process.env['E2E_GROUP'];
const group = groups.find((candidate) => candidate.name === selected);
if (!group) {
  throw new Error(
    `set E2E_GROUP to one of: ${groups.map((candidate) => candidate.name).join(', ')}`,
  );
}
const entries = entriesOf(group);
// A group's only case can be the one case that is not buildable yet.
const only = Object.keys(entries).length === 1 ? Object.keys(entries)[0] : undefined;

export default defineConfig({
  plugins: [pluginReactLynx({ engineVersion: ENGINE_VERSION, ...group.options })],
  mode: group.mode,
  source: { entry: {} },
  output: {
    // Every group writes into the same tree, one `<case>.web.bundle` per card,
    // which is the layout upstream's Playwright goldens were taken against and
    // the one the census reads.
    distPath: {
      root: group.directory ? `dist/${group.directory}`
        : group.ownDirectory && only ? `dist/${only}`
          : 'dist',
    },
    cleanDistPath: false,
    assetPrefix: group.assetPrefix
      ?? (group.ownDirectory && only ? `${ASSET_PREFIX}/${only}` : ASSET_PREFIX),
  },
  // A vendor chunk shared between cards upstream keeps apart would change what
  // each card loads, so everything is one chunk unless the case is about
  // splitting, where the preset comes from the table.
  splitChunks: group.splitChunks ?? false,
  environments: {
    web: { source: { entry: entries } },
  },
});
