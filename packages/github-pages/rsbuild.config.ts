import { defineConfig } from '@rsbuild/core';
import { readFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const pagesDirectory = path.dirname(fileURLToPath(import.meta.url));
const packageDirectory = path.resolve(
  pagesDirectory,
  '../../crates/bobcat-wasm',
);
const showcaseDirectory = path.resolve(pagesDirectory, '../explorer-showcase');

// The showcase menus the Explorer homepage navigates to, one bundle per
// ReactLynx entry, published as `showcase/menu/<name>.web.bundle`.
const showcaseMenus = path.join(showcaseDirectory, 'dist');

// The demo categories, taken from the showcase's own `@lynx-example`
// dependencies rather than restated here: each package name's unscoped half is
// exactly the directory the menus link to (`showcase/scroll-view/…`).
const showcasePackage = JSON.parse(
  readFileSync(path.join(showcaseDirectory, 'package.json'), 'utf8'),
) as { dependencies: Record<string, string> };
// pnpm's strict layout puts these under the showcase's own `node_modules`, so
// they are resolved from its manifest rather than from this package.
const requireFromShowcase = createRequire(
  path.join(showcaseDirectory, 'package.json'),
);
const showcaseCategories = Object.keys(showcasePackage.dependencies)
  .filter((name) => name.startsWith('@lynx-example/'))
  .map((name) => ({
    // The whole `dist/`: both bundle flavours and the `static/` images and
    // fonts a demo names relative to itself, which only resolve if the tree
    // is published the way the package ships it.
    from: path.join(
      path.dirname(requireFromShowcase.resolve(`${name}/package.json`)),
      'dist',
    ),
    to: path.posix.join('showcase', name.slice('@lynx-example/'.length)),
  }));
// These modules execute as native ESM, so the npm package allowlist is also
// the Pages asset manifest. A new facade dependency then reaches both outputs.
const browserFiles = (
  JSON.parse(
    readFileSync(path.join(packageDirectory, 'package.json'), 'utf8'),
  ) as { files: string[] }
).files;

function pagesBasePath(value: string | undefined): string {
  const segments = (value ?? '')
    .trim()
    .split('/')
    .filter((segment) => segment.length > 0);

  return segments.length === 0 ? '/' : `/${segments.join('/')}/`;
}

const basePath = pagesBasePath(process.env['PAGES_BASE_PATH']);

export default defineConfig({
  server: {
    base: basePath,
  },
  dev: {
    assetPrefix: basePath,
  },
  output: {
    assetPrefix: basePath,
  },
  environments: {
    web: {
      source: {
        entry: {
          index: './src/index.ts',
        },
      },
      output: {
        // wasm_thread imports the generated glue by its real URL. Keep this
        // small package as native ESM instead of letting Rspack inline
        // import.meta.url as a build-machine file URL.
        copy: [
          {
            from: path.resolve(
              packageDirectory,
              '../hughie/tests/fixtures/Roboto-Regular.ttf',
            ),
            to: 'Roboto-Regular.ttf',
            info: { minimized: true },
          },
          {
            // The template the Canvas tab loads first.
            from: path.join(
              pagesDirectory,
              '../explorer-homepage/dist/main.web.bundle',
            ),
            to: 'explorer-homepage/main.web.bundle',
            info: { minimized: true },
          },
          {
            // The homepage's showcase menus. `ExplorerModule.openSchema` is
            // handed `showcase/menu/<name>.lynx.bundle` and loads the web
            // sibling, so only that flavour is published here.
            from: path.join(showcaseMenus, '*.web.bundle'),
            to: 'showcase/menu/[name][ext]',
            info: { minimized: true },
          },
          ...showcaseCategories.map(({ from, to }) => ({
            from,
            to,
            info: { minimized: true },
          })),
          ...browserFiles.map((file) => ({
            from: path.join(packageDirectory, file),
            to: path.posix.join('bobcat-wasm', file),
            // `dist/` also holds the compiler's incremental build info.
            globOptions: { ignore: ['**/*.tsbuildinfo'] },
            info: { minimized: true },
          })),
        ],
      },
    },
    // The page registers this Service Worker by a fixed URL beside it, so it
    // is built as a classic Worker script with no hash and no directory.
    'coi-service-worker': {
      source: {
        entry: {
          'coi-service-worker': './src/coi-service-worker.ts',
        },
      },
      output: {
        target: 'web-worker',
        distPath: {
          js: '',
        },
        filename: {
          js: '[name].js',
        },
      },
    },
  },
  html: {
    title: 'Bobcat · Rust on the web',
    meta: {
      description:
        'A cross-origin-isolated, multithreaded WebAssembly demo for Bobcat.',
      viewport: 'width=device-width, initial-scale=1, viewport-fit=cover',
      'theme-color': '#0d1017',
    },
    tags: [
      {
        tag: 'base',
        attrs: { href: basePath },
        head: true,
        append: false,
        publicPath: false,
      },
    ],
  },
});
