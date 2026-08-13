import { defineConfig } from '@rsbuild/core';

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
