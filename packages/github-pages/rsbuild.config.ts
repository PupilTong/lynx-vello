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
const browserScripts = (
  JSON.parse(
    readFileSync(path.join(packageDirectory, 'package.json'), 'utf8'),
  ) as { files: string[] }
).files.filter((file) => path.extname(file) === '.js');

function pagesBasePath(value: string | undefined): string {
  const segments = (value ?? '')
    .trim()
    .split('/')
    .filter((segment) => segment.length > 0);

  return segments.length === 0 ? '/' : `/${segments.join('/')}/`;
}

const basePath = pagesBasePath(process.env['PAGES_BASE_PATH']);

export default defineConfig({
  source: {
    entry: {
      index: './src/index.ts',
    },
  },
  server: {
    base: basePath,
  },
  dev: {
    assetPrefix: basePath,
  },
  output: {
    assetPrefix: basePath,
    // wasm_thread imports the generated glue by its real URL. Keep this small
    // package as native ESM instead of letting Rspack inline import.meta.url
    // as a build-machine file URL.
    copy: [
      {
        from: path.resolve(
          packageDirectory,
          '../hughie/tests/fixtures/Roboto-Regular.ttf',
        ),
        to: 'Roboto-Regular.ttf',
        info: { minimized: true },
      },
      ...browserScripts.map((file) => ({
        from: path.join(packageDirectory, file),
        to: path.posix.join('bobcat-wasm', file),
        info: { minimized: true },
      })),
      {
        from: path.join(packageDirectory, 'pkg'),
        to: 'bobcat-wasm/pkg',
        info: { minimized: true },
      },
    ],
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
