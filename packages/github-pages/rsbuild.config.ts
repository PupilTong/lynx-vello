import { defineConfig } from '@rsbuild/core';
import { readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const packageDirectory = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  '../../crates/bobcat-wasm',
);
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
